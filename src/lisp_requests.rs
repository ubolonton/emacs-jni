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
    /// A Clojure symbol or keyword (an `emacs_jni.LispSymbol` marker) —
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
        LispRequestArg::Scalar(scalar) => match scalar {
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
                "a Lisp-access apply argument is never classified as LispArg::Callback -- \
                 see LispRequestArg::Scalar's doc comment"
            ),
        },
    }
}

/// Sends a `Cmd::LispReply` for ID, converting an `Err` into
/// `LispReplyValue::Error` (its `Display` text — matches
/// `jvm_thread::handle_invoke_reply`'s use of a plain formatted message
/// for a failed callback).
fn reply(id: u64, result: EmacsResult<LispReplyValue>) {
    let value = match result {
        Ok(v) => v,
        Err(e) => LispReplyValue::Error(format!("{e}")),
    };
    let _ = worker::send_command(Cmd::LispReply { id, value });
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
/// message`'s `(lisp-request)` case (`elisp/emacs-jni.el`).
///
/// Must not reorder or batch-by-kind — see spec "Ordering invariant": a
/// `FreeRef` is only sound to run exactly when it's reached in this order,
/// not eagerly and not deferred to the end of the batch.
pub fn service(env: &Env) -> EmacsResult<()> {
    for req in drain() {
        match req {
            LispRequest::Intern { id, name } => {
                // Routed through `value_to_reply` like every other result:
                // an interned symbol becomes a Clojure symbol, a keyword a
                // keyword, and `intern "nil"`/`"t"` stay `nil`/`true`.
                let result = env.intern(&name).and_then(|v| value_to_reply(env, v));
                reply(id, result);
            }
            LispRequest::Apply { id, f, args } => {
                let result = (|| -> EmacsResult<LispReplyValue> {
                    let f_value = arg_to_value(env, &f)?;
                    let mut arg_values = Vec::with_capacity(args.len());
                    for arg in &args {
                        arg_values.push(arg_to_value(env, arg)?);
                    }
                    let result_value = f_value.call(&arg_values)?;
                    value_to_reply(env, result_value)
                })();
                reply(id, result);
            }
            LispRequest::TypeOf { id, arg } => {
                let result = arg_to_value(env, &arg)
                    .and_then(|v| env.type_of(v))
                    .and_then(|v| value_to_reply(env, v));
                reply(id, result);
            }
            LispRequest::IsNotNil { id, arg } => {
                let result = arg_to_value(env, &arg).map(|v| LispReplyValue::Bool(v.is_not_nil()));
                reply(id, result);
            }
            LispRequest::Eq { id, a, b } => {
                let result = (|| -> EmacsResult<LispReplyValue> {
                    let av = arg_to_value(env, &a)?;
                    let bv = arg_to_value(env, &b)?;
                    Ok(LispReplyValue::Bool(env.call("eq", (av, bv))?.is_not_nil()))
                })();
                reply(id, result);
            }
            LispRequest::Prin1 { id, ptr } => {
                let result = env
                    .call("prin1-to-string", (borrow(env, ptr),))
                    .and_then(|v| v.into_rust::<String>())
                    .map(LispReplyValue::Str);
                reply(id, result);
            }
            LispRequest::FreeRef { ptr } => {
                // Safety: PTR is a `LispObject`'s own `ptr`, sent exactly
                // once by its Cleaner (`LispRefReleaseNotifier.run`, which
                // only ever fires after the object it was registered
                // against is unreachable) — see this module's doc comment
                // and spec "Object model".
                let gref = unsafe { Box::from_raw(ptr as *mut GlobalRef) };
                if let Err(e) = gref.free(env) {
                    channel::send(&protocol::log_message(
                        "error",
                        &format!("jvm--service-lisp-requests: failed to free a LispObject ref: {e}"),
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
