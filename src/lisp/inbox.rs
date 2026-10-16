//! The typed queue for every JVM→Emacs message.
//!
//! A JVM thread has no [`Env`]; only the Emacs main thread has one, during a module call. So a JVM
//! thread calls [`submit`], which enqueues an [`Inbound`] and writes a wakeup byte to the pipe
//! process. The pipe's `:filter`, `jvm--service-inbound`, drains the queue on the main thread.
//! Lisp-access requests reply over [`Cmd::LispReply`]; other messages call an Elisp helper.
//!
//! A `LispObject`'s `ptr` is [`Box::into_raw`] of its [`GlobalRef`]. This module borrows it
//! ([`borrow`]), and [`Inbound::FreeRef`] frees it once.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::Mutex;

use emacs::{Env, GlobalRef, Result as EmacsResult, Value};

use crate::jvm::inbox::{self, Cmd};
use crate::lisp::convert::{
    from_bridge, from_input, signal_message, throw_message, to_bridge, to_outcome,
};
use crate::protocol::{BridgeValue, LispInput, LispOutcome, Status};

/// One JVM→Emacs message. The first seven variants are Lisp-access requests; all except
/// [`Inbound::FreeRef`] get a reply. `id` is the key of the waiting future in
/// [`crate::jvm::inbox::register`]; it is 0 on the direct path, which has no future.
#[derive(Debug)]
pub enum Inbound {
    /// `Lisp.intern`: intern `name` in the global obarray. A leading `:` makes a keyword.
    Intern { id: u64, name: String },
    /// `Lisp.applyAsync`: call `f` with `args`.
    Apply { id: u64, f: LispInput, args: Vec<LispInput> },
    /// `Lisp.typeOf`: `type-of` of `arg`.
    TypeOf { id: u64, arg: LispInput },
    /// `Lisp.isNotNil`: whether `arg` is not `nil`.
    IsNotNil { id: u64, arg: LispInput },
    /// `Lisp.eq`: whether `a` and `b` are `eq`.
    Eq { id: u64, a: LispInput, b: LispInput },
    /// `LispObject.prin1ToString`: `prin1-to-string` of the value at `ptr`. A raw `ptr`, not a
    /// [`LispInput`], because the receiver is always a `LispObject`.
    Prin1 { id: u64, ptr: i64 },
    /// A `LispObject` was collected: free the [`GlobalRef`] at `ptr`. Has no reply. Enqueued by
    /// `LispRefReleaseNotifier.run`.
    FreeRef { ptr: i64 },
    /// A `jvm-call-async`/`jvm-new-async` call finished. Enqueued by
    /// [`crate::jvm::invoke::handle_call`].
    CallDone { id: u64, ok: bool, result: BridgeValue, err: Option<String> },
    /// A JVM-initiated call into a previously wrapped Lisp function (see
    /// `jvm--register-callback`). Enqueued by `jvm::entry::lisp_function_call`.
    Invoke { id: u64, handle: u64, args: Vec<BridgeValue> },
    /// A JVM-side `LispFunction` proxy was garbage collected. Enqueued by
    /// `jvm::entry::release_notifier_run`.
    Release { handle: u64 },
    /// A log line from the worker thread.
    Log { level: String, text: String },
    /// A JVM lifecycle status transition, from the worker thread.
    Status { status: Status, detail: Option<String> },
    /// A `jvm-load-jar-async` call finished. Enqueued by `jvm::inbox::handle_load_jar`.
    LoadJarDone { id: u64, ok: bool, err: Option<String> },
}

/// The pending messages, oldest first. Filled by JVM threads, drained by the main thread.
static QUEUE: Mutex<VecDeque<Inbound>> = Mutex::new(VecDeque::new());

/// Adds `req` to the tail of the queue. Use [`submit`], which also wakes the main thread.
fn enqueue(req: Inbound) {
    QUEUE.lock().unwrap().push_back(req);
}

/// Removes and returns every queued message, in FIFO order.
fn drain() -> Vec<Inbound> {
    QUEUE.lock().unwrap().drain(..).collect()
}

/// The write end of the wakeup pipe. Set by [`init`].
static WRITER: Mutex<Option<Box<dyn Write + Send>>> = Mutex::new(None);

/// Opens a channel to the pipe process `process` and stores its writer, replacing any previous
/// one. Called from `jvm--init-channel`, before any JVM thread can reach [`submit`].
pub fn init(env: &Env, process: Value) -> EmacsResult<()> {
    let writer = env.open_channel(process)?;
    *WRITER.lock().unwrap() = Some(Box::new(writer));
    Ok(())
}

/// Wakes the main thread to drain the queue. Coalesced wakeups are harmless, because the filter
/// drains the whole queue. Write failures are ignored: a JVM thread cannot recover from a broken
/// channel, and the Elisp `-await` timeouts catch a lost wakeup.
fn poke() {
    if let Some(writer) = WRITER.lock().unwrap().as_mut() {
        let _ = writer.write_all(b"\x01");
    }
}

/// Adds `req` to the queue and wakes the main thread to drain it.
pub fn submit(req: Inbound) {
    enqueue(req);
    poke();
}

/// Boxes a [`GlobalRef`] to `value` for a Java `LispObject`, and returns the pointer. An
/// [`Inbound::FreeRef`] frees it.
pub(crate) fn box_value(value: Value) -> i64 {
    Box::into_raw(Box::new(value.make_global_ref())) as i64
}

/// Borrows the [`GlobalRef`] at `ptr` and binds it to `env`.
///
/// Not an `unsafe fn`, because the invariant is process-wide: `ptr` must be the `ptr` of a
/// `LispObject` that was reachable in Java when the request was enqueued. Its
/// [`Inbound::FreeRef`] is enqueued later, and [`drain`] is FIFO, so the ref is still live here.
pub(crate) fn borrow<'e>(env: &'e Env, ptr: i64) -> Value<'e> {
    // SAFETY: `ptr` came from `box_value` and is not freed yet; see the doc comment.
    let gref: &'e GlobalRef = unsafe { &*(ptr as *const GlobalRef) };
    gref.bind(env)
}

/// Sends a [`Cmd::LispReply`] for `id`. An `Err` becomes a [`LispOutcome::Signal`] or
/// [`LispOutcome::Throw`] reply.
fn reply(env: &Env, id: u64, result: EmacsResult<LispOutcome>) {
    let value = match result {
        Ok(v) => v,
        Err(e) => error_to_reply(env, e),
    };
    let _ = inbox::submit(Cmd::LispReply { id, value });
}

/// Converts an [`emacs::Error`] to a [`LispOutcome::Signal`] or [`LispOutcome::Throw`] with the
/// original symbol/data or tag/value. Any other error becomes a `Signal` with symbol `error` and
/// the message as data. `env` must be the [`Env`] of the failed call.
fn error_to_reply(env: &Env, e: emacs::Error) -> LispOutcome {
    use emacs::ErrorKind;
    let boxed = |r: EmacsResult<LispOutcome>| r.ok().map(Box::new);
    match e.downcast_ref::<ErrorKind>() {
        Some(ErrorKind::Signal { symbol, data }) => {
            // SAFETY: `TempValue::value` requires the `Env` that made the error. `env` is the `Env`
            // of the failed call, per this function's contract.
            let symbol = unsafe { symbol.value(env) };
            // SAFETY: as for `symbol`.
            let data = unsafe { data.value(env) };
            let message = signal_message(env, symbol, data);
            let s = boxed(to_outcome(env, symbol));
            let d = boxed(to_outcome(env, data));
            match (s, d) {
                (Some(symbol), Some(data)) => LispOutcome::Signal { symbol, data, message },
                // If only one side converted, it may hold a boxed `GlobalRef`. Free it.
                (s, d) => {
                    free_reply_ptr(env, s);
                    free_reply_ptr(env, d);
                    fallback_signal(&format!("{e}"))
                }
            }
        }
        Some(ErrorKind::Throw { tag, value }) => {
            // SAFETY: as for `Signal` above.
            let tag = unsafe { tag.value(env) };
            // SAFETY: as for `Signal` above.
            let value = unsafe { value.value(env) };
            let message = throw_message(env, tag, value);
            let t = boxed(to_outcome(env, tag));
            let v = boxed(to_outcome(env, value));
            match (t, v) {
                (Some(tag), Some(value)) => LispOutcome::Throw { tag, value, message },
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

/// Frees the boxed [`GlobalRef`] in `v` if it is a [`LispOutcome::Ptr`]. Use on the error path,
/// for a converted value that never reaches Java (so no `Cleaner` will free it).
fn free_reply_ptr(env: &Env, v: Option<Box<LispOutcome>>) {
    if let Some(b) = v {
        if let LispOutcome::Ptr(ptr) = *b {
            // SAFETY: `ptr` came from `box_value` and never reached Java, so this is its only free.
            let gref = unsafe { Box::from_raw(ptr as *mut GlobalRef) };
            let _ = gref.free(env);
        }
    }
}

/// A [`LispOutcome::Signal`] reply for a Rust-internal error: symbol `error`, data the message.
/// `data` is a string, not a list, because this path has no [`Env`] to build a list. The Java
/// message is `message` itself.
fn fallback_signal(message: &str) -> LispOutcome {
    LispOutcome::Signal {
        symbol: Box::new(LispOutcome::Value(BridgeValue::Symbol {
            name: "error".to_string(),
            keyword: false,
        })),
        data: Box::new(LispOutcome::Value(BridgeValue::Str(message.to_string()))),
        message: Some(message.to_string()),
    }
}

/// Drains the queue and runs each message, in order. Emacs calls this through
/// `jvm--service-inbound` on every wakeup byte.
///
/// Do not reorder. An [`Inbound::FreeRef`] is sound only at its position in the queue: an earlier
/// request may still borrow the same ref. `Log` and `Status` must also stay in order.
///
/// A failing message must not abort the batch: the drained messages are already off the queue.
/// Non-Lisp-access messages run user callbacks and hooks, which can signal; their errors are
/// logged. Lisp-access errors become signal/throw replies.
pub fn service_inbound(env: &Env) -> EmacsResult<()> {
    for req in drain() {
        match &req {
            Inbound::FreeRef { ptr } => {
                // SAFETY: `ptr` is a `LispObject`'s `ptr`, sent once by its `Cleaner` after the
                // object became unreachable. Earlier borrows of it drained before this.
                let gref = unsafe { Box::from_raw(*ptr as *mut GlobalRef) };
                if let Err(e) = gref.free(env) {
                    submit(Inbound::Log {
                        level: "error".to_string(),
                        text: format!("jvm--service-inbound: failed to free a LispObject ref: {e}"),
                    });
                }
            }
            // The direct path (`compute_reply`) runs the same op.
            Inbound::Intern { id, .. }
            | Inbound::Apply { id, .. }
            | Inbound::TypeOf { id, .. }
            | Inbound::IsNotNil { id, .. }
            | Inbound::Eq { id, .. }
            | Inbound::Prin1 { id, .. } => {
                let id = *id;
                let result = run_request_value(env, &req);
                reply(env, id, result);
            }
            Inbound::CallDone { .. }
            | Inbound::Invoke { .. }
            | Inbound::Release { .. }
            | Inbound::Log { .. }
            | Inbound::Status { .. }
            | Inbound::LoadJarDone { .. } => {
                if let Err(e) = handle_message(env, &req) {
                    submit(Inbound::Log {
                        level: "error".to_string(),
                        text: format!("jvm--service-inbound: failed to handle {req:?}: {e}"),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Runs one non-Lisp-access message.
fn handle_message(env: &Env, request: &Inbound) -> EmacsResult<()> {
    match request {
        Inbound::CallDone { id, ok, result, err } => {
            let value = from_bridge(env, result)?;
            env.call("jvm--resolve-call", (*id as i64, *ok, value, err.as_deref()))?;
        }
        Inbound::Invoke { id, handle, args } => {
            let mut arg_values = Vec::with_capacity(args.len());
            for arg in args {
                arg_values.push(from_bridge(env, arg)?);
            }
            let args_list = env.list(&arg_values)?;
            env.call("jvm--resolve-invoke", (*id as i64, *handle as i64, args_list))?;
        }
        Inbound::Release { handle } => {
            env.call("jvm--drop-callback", (*handle as i64,))?;
        }
        Inbound::Log { level, text } => {
            env.call("jvm--log-message", (level.as_str(), text.as_str()))?;
        }
        Inbound::Status { status, detail } => {
            env.call("jvm--apply-status", (status.as_tag(), detail.as_deref()))?;
        }
        Inbound::LoadJarDone { id, ok, err } => {
            env.call("jvm--resolve-load-jar", (*id as i64, *ok, err.as_deref()))?;
        }
        Inbound::FreeRef { .. }
        | Inbound::Intern { .. }
        | Inbound::Apply { .. }
        | Inbound::TypeOf { .. }
        | Inbound::IsNotNil { .. }
        | Inbound::Eq { .. }
        | Inbound::Prin1 { .. } => {
            unreachable!("handle_message only receives non-Lisp-access variants")
        }
    }
    Ok(())
}

/// Runs a Lisp-access request (except [`Inbound::FreeRef`]). Used by both the queue and the direct
/// path.
fn run_request_value(env: &Env, request: &Inbound) -> EmacsResult<LispOutcome> {
    match request {
        // `to_outcome` maps `nil`/`t` to `nil`/`true`.
        Inbound::Intern { name, .. } => env.intern(name).and_then(|v| to_outcome(env, v)),
        Inbound::Apply { f, args, .. } => {
            let f_value = from_input(env, f)?;
            let mut arg_values = Vec::with_capacity(args.len());
            for arg in args {
                arg_values.push(from_input(env, arg)?);
            }
            to_outcome(env, f_value.call(&arg_values)?)
        }
        Inbound::TypeOf { arg, .. } => {
            from_input(env, arg).and_then(|v| env.type_of(v)).and_then(|v| to_outcome(env, v))
        }
        Inbound::IsNotNil { arg, .. } => {
            from_input(env, arg).map(|v| LispOutcome::Bool(v.is_not_nil()))
        }
        Inbound::Eq { a, b, .. } => {
            let av = from_input(env, a)?;
            let bv = from_input(env, b)?;
            Ok(LispOutcome::Bool(env.call("eq", (av, bv))?.is_not_nil()))
        }
        Inbound::Prin1 { ptr, .. } => env
            .call("prin1-to-string", (borrow(env, *ptr),))
            .and_then(|v| v.into_rust::<String>())
            .map(|s| LispOutcome::Value(BridgeValue::Str(s))),
        Inbound::FreeRef { .. } => {
            unreachable!("FreeRef has no reply and never takes the direct path")
        }
        Inbound::CallDone { .. }
        | Inbound::Invoke { .. }
        | Inbound::Release { .. }
        | Inbound::Log { .. }
        | Inbound::Status { .. }
        | Inbound::LoadJarDone { .. } => {
            unreachable!("run_request_value only receives Lisp-access variants")
        }
    }
}

/// Runs a Lisp-access request directly, for a `Lisp.*` native called inside `jvm--call-direct`.
/// The queue would deadlock there, because this thread is the one that drains it. Errors become
/// [`LispOutcome::Signal`]/[`LispOutcome::Throw`], as in [`reply`].
pub fn compute_reply(env: &Env, request: &Inbound) -> LispOutcome {
    match run_request_value(env, request) {
        Ok(v) => v,
        Err(e) => error_to_reply(env, e),
    }
}

/// Calls the Lisp callback registered under `handle` with `args`, for a `LispFunction.call` made
/// inside `jvm--call-direct`. The queue would deadlock there, as in [`compute_reply`]. Errors
/// become [`LispOutcome::Signal`]/[`LispOutcome::Throw`], as on the async path
/// (`jvm--resolve-invoke`).
pub fn invoke_callback(env: &Env, handle: u64, args: &[BridgeValue]) -> LispOutcome {
    match run_callback(env, handle, args) {
        Ok(v) => v,
        Err(e) => error_to_reply(env, e),
    }
}

/// Looks up `handle` in `jvm--lisp-callbacks` and calls the function with `args`. Signals `error`
/// for an unknown handle.
fn run_callback(env: &Env, handle: u64, args: &[BridgeValue]) -> EmacsResult<LispOutcome> {
    let table = env.call("symbol-value", (env.intern("jvm--lisp-callbacks")?,))?;
    let function = env.call("gethash", (handle as i64, table))?;
    if !function.is_not_nil() {
        // `error` always signals, so `?` returns here.
        env.call("error", ("no such callback handle %s", handle as i64))?;
    }
    let mut arg_values = Vec::with_capacity(args.len());
    for arg in args {
        arg_values.push(from_bridge(env, arg)?);
    }
    Ok(LispOutcome::Value(to_bridge(function.call(&arg_values)?)?))
}

#[cfg(test)]
mod tests {
    use std::sync::{MutexGuard, PoisonError};

    use super::*;

    /// Serializes the tests that use the process-global [`QUEUE`]. cargo runs tests in parallel,
    /// so without it a test can drain another test's messages.
    static QUEUE_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Locks [`QUEUE_TEST_LOCK`] and empties the queue. Hold the guard for the whole test.
    fn lock_queue() -> MutexGuard<'static, ()> {
        // A failed test poisons the lock. The other tests are still valid.
        let guard = QUEUE_TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        drain();
        guard
    }

    #[test]
    fn fallback_signal_is_error_symbol_with_message() {
        match fallback_signal("boom") {
            LispOutcome::Signal { symbol, data, message } => {
                assert!(matches!(
                    *symbol,
                    LispOutcome::Value(BridgeValue::Symbol { ref name, keyword: false }) if name == "error"
                ));
                assert!(
                    matches!(*data, LispOutcome::Value(BridgeValue::Str(ref s)) if s == "boom")
                );
                assert_eq!(message.as_deref(), Some("boom"));
            }
            _ => panic!("expected Signal"),
        }
    }

    #[test]
    fn drain_returns_requests_in_fifo_order() {
        let _queue = lock_queue();
        // `FreeRef` is the simplest variant. No `Env`, so nothing is freed.
        enqueue(Inbound::FreeRef { ptr: 1 });
        enqueue(Inbound::FreeRef { ptr: 2 });
        enqueue(Inbound::FreeRef { ptr: 3 });
        let drained = drain();
        let ptrs: Vec<i64> = drained
            .iter()
            .map(|r| match r {
                Inbound::FreeRef { ptr } => *ptr,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(ptrs, vec![1, 2, 3]);
    }

    #[test]
    fn drain_empties_the_queue() {
        let _queue = lock_queue();
        enqueue(Inbound::FreeRef { ptr: 42 });
        assert_eq!(drain().len(), 1);
        assert!(drain().is_empty());
    }

    /// `FreeRef` must drain after an earlier request that borrows the same ptr.
    #[test]
    fn apply_enqueued_before_freeref_for_the_same_ptr_drains_before_it() {
        let _queue = lock_queue();
        enqueue(Inbound::Apply { id: 1, f: LispInput::Obj(99), args: vec![] });
        enqueue(Inbound::FreeRef { ptr: 99 });
        let drained = drain();
        assert_eq!(drained.len(), 2);
        assert!(matches!(drained[0], Inbound::Apply { f: LispInput::Obj(99), .. }));
        assert!(matches!(drained[1], Inbound::FreeRef { ptr: 99 }));
    }

    #[test]
    fn message_variants_drain_in_fifo_order() {
        let _queue = lock_queue();
        enqueue(Inbound::CallDone { id: 1, ok: true, result: BridgeValue::Nil, err: None });
        enqueue(Inbound::Invoke { id: 2, handle: 3, args: vec![] });
        enqueue(Inbound::Release { handle: 4 });
        enqueue(Inbound::Log { level: "info".to_string(), text: "x".to_string() });
        enqueue(Inbound::Status { status: Status::Ready, detail: None });
        enqueue(Inbound::LoadJarDone { id: 5, ok: true, err: None });

        let drained = drain();
        assert_eq!(drained.len(), 6);
        assert!(matches!(
            drained[0],
            Inbound::CallDone { id: 1, ok: true, result: BridgeValue::Nil, err: None }
        ));
        assert!(matches!(drained[1], Inbound::Invoke { id: 2, handle: 3, .. }));
        assert!(matches!(drained[2], Inbound::Release { handle: 4 }));
        assert!(matches!(drained[3], Inbound::Log { .. }));
        assert!(matches!(drained[4], Inbound::Status { status: Status::Ready, detail: None }));
        assert!(matches!(drained[5], Inbound::LoadJarDone { id: 5, ok: true, err: None }));
    }
}
