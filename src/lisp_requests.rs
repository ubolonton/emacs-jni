//! The Rust-side half of "Clojure holds and manipulates Lisp values" (see
//! design spec `docs/superpowers/specs/2026-09-08-clojure-lisp-access-design.md`).
//!
//! A JVM thread (a `Lisp.*` native method, `src/shim.rs`) never has an
//! `emacs::Env` — that only exists on the Emacs main thread, during a
//! module call. So a native method enqueues a `LispRequest` here and wakes
//! Emacs up (`(lisp-request)`, over `channel::send`); `jvm--service-lisp-
//! requests` (`lib.rs`) drains this queue on the main thread, via
//! `service` below, and replies over the worker's `Cmd::LispReply`.
//!
//! No handle table: a `LispObject`'s `ptr` field IS `Box::into_raw` of its
//! `emacs::GlobalRef` — this module only borrows it (`borrow`) or, for
//! `FreeRef`, reclaims and frees it exactly once.

use std::collections::VecDeque;
use std::sync::Mutex;

use emacs::{Env, GlobalRef, IntoLisp, Result as EmacsResult, Value};

use crate::commands::{Cmd, LispArg, LispReplyValue};
use crate::{channel, protocol, worker};

/// One value crossing Clojure → Lisp — an `apply` receiver or argument, or
/// the operand of `typeOf`/`isNotNil`/`eq` — already classified on the JVM
/// thread (`shim::object_to_lisp_request_arg`). See spec "Argument
/// conversion".
#[derive(Debug)]
pub enum LispRequestArg {
    /// A `LispObject`, carried by its `ptr` — bound to a `Value` on the
    /// main thread, never freed here (see `borrow`).
    Obj(i64),
    /// A Clojure symbol or keyword (an `emacs_jni.core.LispSymbol` marker) —
    /// interned on the main thread. `name` is the bare name; a keyword is
    /// interned with a leading `:`.
    Symbol { name: String, keyword: bool },
    /// A scalar or other-JVM-object argument, already reduced to a
    /// `LispArg` the same way an ordinary `jvm-call` argument would be
    /// (reuses `marshal::jvm_value_to_lisp_arg`) — `LispArg::Callback`
    /// never appears here (that variant is only ever produced from the
    /// Lisp-to-Java direction in `lib.rs`'s `lisp_value_to_arg`).
    Scalar(LispArg),
}

/// A single request from a JVM thread to run one `Env` op on the main
/// thread. `FreeRef` carries no `id` and produces no reply (see spec
/// "Request/reply flow"). `Prin1` alone keeps a raw `ptr`: it is
/// `LispObject.prin1ToString`, so its receiver is always a real
/// `LispObject`, never a symbol/scalar.
#[derive(Debug)]
pub enum LispRequest {
    Intern { id: u64, name: String },
    Apply { id: u64, f: LispRequestArg, args: Vec<LispRequestArg> },
    TypeOf { id: u64, arg: LispRequestArg },
    IsNotNil { id: u64, arg: LispRequestArg },
    Eq { id: u64, a: LispRequestArg, b: LispRequestArg },
    Prin1 { id: u64, ptr: i64 },
    FreeRef { ptr: i64 },
}

static QUEUE: Mutex<VecDeque<LispRequest>> = Mutex::new(VecDeque::new());

/// Adds REQ to the tail of the queue. Called from a JVM thread (inside a
/// `Lisp.*`/`LispRefReleaseNotifier` native), before waking the main
/// thread with `(lisp-request)`.
pub fn enqueue(req: LispRequest) {
    QUEUE.lock().unwrap().push_back(req);
}

/// Removes and returns every currently-queued request, in FIFO order.
/// Requests enqueued *during* `service`'s processing of this batch are not
/// included — they wait for the next `(lisp-request)` wakeup, which is
/// harmless (see spec "Ordering invariant": what matters is that entries
/// already in the queue are drained in the order they were pushed, not
/// that every request enqueued before `service` returns gets included in
/// the same batch).
fn drain() -> Vec<LispRequest> {
    QUEUE.lock().unwrap().drain(..).collect()
}

/// Boxes VALUE's `GlobalRef`, handing ownership to the eventual Java
/// `LispObject` — freed exactly once, later, by a `FreeRef` request (see
/// `service`'s own `LispRequest::FreeRef` arm).
fn box_value(value: Value) -> i64 {
    Box::into_raw(Box::new(value.make_global_ref())) as i64
}

/// Borrows the `GlobalRef` at PTR and binds it to ENV.
///
/// # Safety (not a `unsafe fn` — the invariant is process-wide, not
/// caller-local)
///
/// PTR must be a live `LispObject`'s `ptr`. Every caller here reads PTR
/// from a request argument that names a `LispObject` still reachable on
/// the Java side for the duration of the call that enqueued the request
/// (the object itself, or one of `apply`'s arguments) — so its Cleaner-
/// driven `FreeRef` cannot have run yet, and strict FIFO draining
/// (`drain`) guarantees any `FreeRef` for the same ptr that *is* already
/// queued was enqueued after this request, not before. See spec "Ordering
/// invariant".
fn borrow<'e>(env: &'e Env, ptr: i64) -> Value<'e> {
    let gref: &'e GlobalRef = unsafe { &*(ptr as *const GlobalRef) };
    gref.bind(env)
}

/// Converts one already-classified apply argument to a real `Value`,
/// mirroring spec "Argument conversion" for the main-thread side.
fn arg_to_value<'e>(env: &'e Env, arg: &LispRequestArg) -> EmacsResult<Value<'e>> {
    match arg {
        LispRequestArg::Obj(ptr) => Ok(borrow(env, *ptr)),
        // A keyword interns with its leading `:`; a plain symbol by name.
        // `env.intern` uses the global obarray, so the result round-trips
        // `eq`-stably back to a Clojure symbol via `value_to_reply`.
        LispRequestArg::Symbol { name, keyword } => {
            if *keyword {
                env.intern(&format!(":{name}"))
            } else {
                env.intern(name)
            }
        }
        LispRequestArg::Scalar(scalar) => lisp_arg_to_value(env, scalar),
    }
}

/// Converts a marshalled `LispArg` to a real Lisp `Value` on the main thread.
///
/// Shared by the Lisp-access apply path (`arg_to_value`'s `Scalar` arm) and
/// the synchronous `jvm--call-direct` return path (`lib.rs`), which turns the
/// `LispArg` `jvm_thread::call_direct` produced into the value the defun
/// returns — the direct-call equivalent of the async path's sexp round trip
/// through `marshal::lisp_arg_to_sexp` + Lisp's reader.
pub fn lisp_arg_to_value<'e>(env: &'e Env, arg: &LispArg) -> EmacsResult<Value<'e>> {
    match arg {
        LispArg::Nil => ().into_lisp(env),
        LispArg::True => true.into_lisp(env),
        LispArg::Int(n) => n.into_lisp(env),
        LispArg::Float(f) => f.into_lisp(env),
        // Emacs's own reader parses an arbitrary-precision decimal
        // string into a bignum when it doesn't fit a fixnum — reusing
        // it here avoids reimplementing bignum construction.
        LispArg::BigInt(digits) => env.call("string-to-number", (digits.as_str(),)),
        LispArg::Str(s) => s.as_str().into_lisp(env),
        // Round-trips through the same `jvm--wrap-handle` primitive an
        // ordinary `call-done`/`invoke` result does, rather than
        // hand-rolling a second "build a JvmHandle user-ptr" path.
        LispArg::Object(handle) => env.call("jvm--wrap-handle", (*handle as i64,)),
        LispArg::Callback(_) => unreachable!(
            "neither a Lisp-access apply argument nor a JVM-call return value is ever \
             classified as LispArg::Callback -- Callback is only produced on the \
             Lisp-to-Java argument direction (see lib.rs's lisp_value_to_arg)"
        ),
    }
}

/// Sends a `Cmd::LispReply` for ID. An `Err` is converted to a `Signal`
/// or `Throw` reply via `error_to_reply` (marshaling the signal symbol/
/// data or throw tag/value through `value_to_reply`).
fn reply(env: &Env, id: u64, result: EmacsResult<LispReplyValue>) {
    let value = match result {
        Ok(v) => v,
        Err(e) => error_to_reply(env, e),
    };
    let _ = worker::send_command(Cmd::LispReply { id, value });
}

/// Converts an `emacs::Error` into a `LispReplyValue::{Signal,Throw}`. A
/// `Signal`/`Throw` `ErrorKind` carries the real symbol/data or tag/value
/// (each a `TempValue` bound with this `Env` -- sound here because `reply`
/// runs on the main thread with the `Env` the failing call used). Anything
/// else (a Rust-internal error, or a marshaling failure while building the
/// nested values) collapses to a `Signal` whose symbol is `error` and
/// whose data is the formatted message string.
fn error_to_reply(env: &Env, e: emacs::Error) -> LispReplyValue {
    use emacs::ErrorKind;
    let boxed = |r: EmacsResult<LispReplyValue>| r.ok().map(Box::new);
    match e.downcast_ref::<ErrorKind>() {
        Some(ErrorKind::Signal { symbol, data }) => {
            let s = boxed(value_to_reply(env, unsafe { symbol.value(env) }));
            let d = boxed(value_to_reply(env, unsafe { data.value(env) }));
            match (s, d) {
                (Some(symbol), Some(data)) => LispReplyValue::Signal { symbol, data },
                // One side may have marshaled to a `Ptr` (already `box_value`d
                // a `GlobalRef`) while the other failed. That surviving box is
                // dropped here, so free its ref first to avoid a leak.
                (s, d) => {
                    free_reply_ptr(env, s);
                    free_reply_ptr(env, d);
                    fallback_signal(&format!("{e}"))
                }
            }
        }
        Some(ErrorKind::Throw { tag, value }) => {
            let t = boxed(value_to_reply(env, unsafe { tag.value(env) }));
            let v = boxed(value_to_reply(env, unsafe { value.value(env) }));
            match (t, v) {
                (Some(tag), Some(value)) => LispReplyValue::Throw { tag, value },
                (t, v) => {
                    free_reply_ptr(env, t);
                    free_reply_ptr(env, v);
                    fallback_signal(&format!("{e}"))
                }
            }
        }
        _ => fallback_signal(&format!("{e}")),
    }
}

/// Reclaims and frees the boxed `GlobalRef` inside V if it is a `Ptr` — for
/// the error-fallback path, where a marshaled component (an `Ok(Ptr)` that
/// already ran `box_value`) is discarded instead of being handed to Java. No
/// Java `LispObject`/Cleaner was ever created for this ptr, so this is the
/// one and only free of it. A `None` or non-`Ptr` value is a no-op.
fn free_reply_ptr(env: &Env, v: Option<Box<LispReplyValue>>) {
    if let Some(b) = v {
        if let LispReplyValue::Ptr(ptr) = *b {
            let gref = unsafe { Box::from_raw(ptr as *mut GlobalRef) };
            let _ = gref.free(env);
        }
    }
}

/// A `Signal` reply for a non-`ErrorKind` error: symbol `error`, data the
/// message string. Keeps every failure arriving at Clojure as a signal.
/// Divergence from spec §2 by intent: `data` is a bare string, not a
/// one-element Lisp list -- this path has no `Env` to build a list and fires
/// only for Rust-internal errors, never a genuine Lisp signal.
fn fallback_signal(message: &str) -> LispReplyValue {
    LispReplyValue::Signal {
        symbol: Box::new(LispReplyValue::Symbol { name: "error".to_string(), keyword: false }),
        data: Box::new(LispReplyValue::Str(message.to_string())),
    }
}

/// Classifies an `apply` result: a scalar or string converts to the
/// matching Clojure value directly; everything else (symbols, conses,
/// vectors, hash tables, ...) stays an opaque `LispObject`. `nil` becomes
/// Clojure `nil` and the symbol `t` becomes `true`, so a Lisp predicate's
/// result is usable directly in a Clojure conditional. The scalar set and
/// its Java shapes mirror the `jvm-call` return-marshalling table.
fn value_to_reply(env: &Env, value: Value) -> EmacsResult<LispReplyValue> {
    if !value.is_not_nil() {
        return Ok(LispReplyValue::Nil);
    }
    if env.call("eq", (value, env.intern("t")?))?.is_not_nil() {
        return Ok(LispReplyValue::Bool(true));
    }
    if env.call("stringp", (value,))?.is_not_nil() {
        return Ok(LispReplyValue::Str(value.into_rust::<String>()?));
    }
    if env.call("floatp", (value,))?.is_not_nil() {
        return Ok(LispReplyValue::Float(value.into_rust::<f64>()?));
    }
    if env.call("integerp", (value,))?.is_not_nil() {
        // A fixnum fits an i64; a bignum does not, so render it to its
        // decimal string and rebuild it as a BigInteger on the JVM side.
        if env.call("fixnump", (value,))?.is_not_nil() {
            return Ok(LispReplyValue::Int(value.into_rust::<i64>()?));
        }
        let digits = env.call("number-to-string", (value,))?.into_rust::<String>()?;
        return Ok(LispReplyValue::BigInt(digits));
    }
    // A keyword's `symbol-name` includes the leading `:`; carry the bare
    // name. `keywordp` in Emacs already implies interned, so every keyword
    // converts.
    if env.call("keywordp", (value,))?.is_not_nil() {
        let full = env.call("symbol-name", (value,))?.into_rust::<String>()?;
        let name = full.strip_prefix(':').unwrap_or(&full).to_string();
        return Ok(LispReplyValue::Symbol { name, keyword: true });
    }
    if env.call("symbolp", (value,))?.is_not_nil() {
        // Only an interned symbol (one `eq` to `intern-soft` of its own
        // name, i.e. present in the global obarray) converts to a Clojure
        // symbol. An uninterned symbol (`make-symbol`/`gensym`) stays an
        // opaque `LispObject`, preserving its identity across the round
        // trip — see spec "Result conversion".
        let name = env.call("symbol-name", (value,))?;
        let interned = env.call("intern-soft", (name,))?;
        if env.call("eq", (value, interned))?.is_not_nil() {
            return Ok(LispReplyValue::Symbol { name: name.into_rust::<String>()?, keyword: false });
        }
    }
    Ok(LispReplyValue::Ptr(box_value(value)))
}

/// Drains the request queue and runs each request's `Env` op, in the
/// order `drain` returned them. Registered as `#[defun]
/// jvm--service-lisp-requests` (`lib.rs`), called from `jvm--dispatch-
/// message`'s `(lisp-request)` case (`elisp/jvm.el`).
///
/// Must not reorder or batch-by-kind — see spec "Ordering invariant": a
/// `FreeRef` is only sound to run exactly when it's reached in this order,
/// not eagerly and not deferred to the end of the batch.
pub fn service(env: &Env) -> EmacsResult<()> {
    for req in drain() {
        match &req {
            LispRequest::FreeRef { ptr } => {
                // Safety: PTR is a `LispObject`'s own `ptr`, sent exactly
                // once by its Cleaner (`LispRefReleaseNotifier.run`, which
                // only ever fires after the object it was registered
                // against is unreachable) — see this module's doc comment
                // and spec "Object model".
                let gref = unsafe { Box::from_raw(*ptr as *mut GlobalRef) };
                if let Err(e) = gref.free(env) {
                    channel::send(&protocol::log_message(
                        "error",
                        &format!("jvm--service-lisp-requests: failed to free a LispObject ref: {e}"),
                    ));
                }
            }
            // Every reply-bearing request runs the same `Env` op as the
            // reentrant direct path (`run_request_value`); the only
            // difference is that here the worker sends the reply back over
            // `Cmd::LispReply`, and there the `Lisp.*` native consumes it in
            // place. See `compute_reply`.
            LispRequest::Intern { id, .. }
            | LispRequest::Apply { id, .. }
            | LispRequest::TypeOf { id, .. }
            | LispRequest::IsNotNil { id, .. }
            | LispRequest::Eq { id, .. }
            | LispRequest::Prin1 { id, .. } => {
                let id = *id;
                let result = run_request_value(env, &req);
                reply(env, id, result);
            }
        }
    }
    Ok(())
}

/// Runs a reply-bearing request's actual `Env` op (everything except
/// `FreeRef`). Shared by `service` (async: the worker returns the reply over
/// the channel) and `compute_reply` (the reentrant direct path).
fn run_request_value(env: &Env, request: &LispRequest) -> EmacsResult<LispReplyValue> {
    match request {
        // Routed through `value_to_reply` like every other result: an
        // interned symbol becomes a Clojure symbol, a keyword a keyword, and
        // `intern "nil"`/`"t"` stay `nil`/`true`.
        LispRequest::Intern { name, .. } => env.intern(name).and_then(|v| value_to_reply(env, v)),
        LispRequest::Apply { f, args, .. } => {
            let f_value = arg_to_value(env, f)?;
            let mut arg_values = Vec::with_capacity(args.len());
            for arg in args {
                arg_values.push(arg_to_value(env, arg)?);
            }
            value_to_reply(env, f_value.call(&arg_values)?)
        }
        LispRequest::TypeOf { arg, .. } => arg_to_value(env, arg)
            .and_then(|v| env.type_of(v))
            .and_then(|v| value_to_reply(env, v)),
        LispRequest::IsNotNil { arg, .. } => {
            arg_to_value(env, arg).map(|v| LispReplyValue::Bool(v.is_not_nil()))
        }
        LispRequest::Eq { a, b, .. } => {
            let av = arg_to_value(env, a)?;
            let bv = arg_to_value(env, b)?;
            Ok(LispReplyValue::Bool(env.call("eq", (av, bv))?.is_not_nil()))
        }
        LispRequest::Prin1 { ptr, .. } => env
            .call("prin1-to-string", (borrow(env, *ptr),))
            .and_then(|v| v.into_rust::<String>())
            .map(LispReplyValue::Str),
        LispRequest::FreeRef { .. } => unreachable!(
            "FreeRef has no reply: it is handled inline in `service`, and never takes the \
             reentrant direct path (a Cleaner thread pushes no `reentry` scope)"
        ),
    }
}

/// The single `LispReplyValue` a reentrant direct-path request produces: the
/// `run_request_value` result, with a Lisp signal/throw folded into a
/// `Signal`/`Throw` reply exactly as the async `reply` path does via
/// `error_to_reply`. Called by the `Lisp.*` natives (`shim.rs`) when the
/// request originates on the main thread inside a `jvm--call-direct`, so it
/// must run directly instead of enqueuing (which would deadlock the blocked
/// main thread). The native then turns this into a Java object or a thrown
/// `LispNonLocalExit` (`jvm_thread::build_lisp_reply_object` /
/// `throw_non_local_exit`).
pub fn compute_reply(env: &Env, request: &LispRequest) -> LispReplyValue {
    match run_request_value(env, request) {
        Ok(v) => v,
        Err(e) => error_to_reply(env, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_signal_is_error_symbol_with_message() {
        match fallback_signal("boom") {
            LispReplyValue::Signal { symbol, data } => {
                assert!(matches!(*symbol, LispReplyValue::Symbol { ref name, keyword: false } if name == "error"));
                assert!(matches!(*data, LispReplyValue::Str(ref s) if s == "boom"));
            }
            _ => panic!("expected Signal"),
        }
    }

    #[test]
    fn drain_returns_requests_in_fifo_order() {
        // Uses `FreeRef` (the variant with the fewest fields) purely as a
        // distinguishable marker; this test never calls `service` (no
        // `Env` here), so no real free happens.
        enqueue(LispRequest::FreeRef { ptr: 1 });
        enqueue(LispRequest::FreeRef { ptr: 2 });
        enqueue(LispRequest::FreeRef { ptr: 3 });
        let drained = drain();
        let ptrs: Vec<i64> = drained
            .iter()
            .map(|r| match r {
                LispRequest::FreeRef { ptr } => *ptr,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(ptrs, vec![1, 2, 3]);
    }

    #[test]
    fn drain_empties_the_queue() {
        enqueue(LispRequest::FreeRef { ptr: 42 });
        assert_eq!(drain().len(), 1);
        assert!(drain().is_empty());
    }

    /// The "Ordering invariant" itself: an `Apply` that borrows a ptr, and
    /// a `FreeRef` for that same ptr enqueued right after it (the shape
    /// that matters is "Apply before FreeRef", exactly the order a real
    /// caller can produce -- see spec "Ordering invariant"), must drain in
    /// that same order so the borrow is never dangling when the op runs.
    #[test]
    fn apply_enqueued_before_freeref_for_the_same_ptr_drains_before_it() {
        enqueue(LispRequest::Apply { id: 1, f: LispRequestArg::Obj(99), args: vec![] });
        enqueue(LispRequest::FreeRef { ptr: 99 });
        let drained = drain();
        assert_eq!(drained.len(), 2);
        assert!(matches!(drained[0], LispRequest::Apply { f: LispRequestArg::Obj(99), .. }));
        assert!(matches!(drained[1], LispRequest::FreeRef { ptr: 99 }));
    }

    /// Box/borrow/free discipline, without a real `Env` (this crate's
    /// `emacs::Value`/`GlobalRef` always need one, so this test exercises
    /// only the pointer plumbing `box_value`/`borrow`'s safety comment
    /// documents: create, borrow (read-only, doesn't consume), then free
    /// exactly once). Uses a plain heap `i64` standing in for a
    /// `GlobalRef`'s own representation (a single `emacs_value`, i.e. one
    /// pointer-sized field) to prove `Box::into_raw`/`Box::from_raw`
    /// round-trip correctly without needing a live JVM/Emacs `Env` in a
    /// `cargo test` unit test.
    #[test]
    fn box_into_raw_then_borrow_then_from_raw_is_exactly_one_free() {
        let boxed: *mut i64 = Box::into_raw(Box::new(7i64));
        let ptr = boxed as i64;

        // "borrow": read without taking ownership, any number of times.
        let borrowed: &i64 = unsafe { &*(ptr as *const i64) };
        assert_eq!(*borrowed, 7);
        let borrowed_again: &i64 = unsafe { &*(ptr as *const i64) };
        assert_eq!(*borrowed_again, 7);

        // "free": reclaim ownership exactly once.
        let reclaimed = unsafe { Box::from_raw(ptr as *mut i64) };
        assert_eq!(*reclaimed, 7);
        // `reclaimed` drops here, exactly once -- a second `Box::from_raw`
        // on the same ptr would be a double-free, which is exactly what
        // the FIFO ordering invariant (tested above) exists to prevent at
        // the request-queue level.
    }
}
