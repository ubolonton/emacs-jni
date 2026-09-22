use emacs::{defun, Env, Result, Value};

emacs::plugin_is_GPL_compatible!();

#[emacs::module(defun_prefix = "", separator = "")]
fn init(env: &Env) -> Result<Value<'_>> {
    // Module load runs on the Emacs main thread; record it so `jvm--call-direct`
    // permanently attaches only this thread to the JVM (stable thread identity),
    // and lets any other caller fall back to a transient scoped attach.
    crate::jvm::runtime::set_main_thread();
    let feature = env.intern("emacs-jni")?;
    env.call("provide", (feature,))
}

#[defun(mod_in_name = false, name = "jvm-ping")]
fn jvm_ping() -> Result<bool> {
    Ok(true)
}

emacs::define_errors! {
    jvm_illegal_transition "JVM operation not allowed in the current state"
    jvm_invalid_argument "Unsupported Lisp value for a JVM call argument"
    jvm_error "JVM call failed"
}

#[defun(mod_in_name = false, name = "jvm--init-channel")]
fn jvm_init_channel(env: &Env, process: Value) -> Result<()> {
    crate::lisp::inbox::init(env, process)
}

#[defun(mod_in_name = false, name = "jvm--start")]
fn jvm_start(env: &Env, classpath: Value, options: Value) -> Result<()> {
    match crate::jvm::runtime::STATUS.begin_start() {
        Ok(()) => {
            crate::jvm::inbox::start_or_resume(string_list(classpath)?, string_list(options)?);
            Ok(())
        }
        Err(current) => env.signal(jvm_illegal_transition, (current.as_tag(),)),
    }
}

#[defun(mod_in_name = false, name = "jvm--pause")]
fn jvm_pause(env: &Env) -> Result<()> {
    match crate::jvm::runtime::STATUS.begin_pause() {
        Ok(()) => match crate::jvm::inbox::submit(crate::jvm::inbox::Cmd::Pause) {
            Ok(()) => Ok(()),
            Err(()) => env.signal(jvm_illegal_transition, ("no running JVM worker thread",)),
        },
        Err(current) => env.signal(jvm_illegal_transition, (current.as_tag(),)),
    }
}

#[defun(mod_in_name = false, name = "jvm--stop")]
fn jvm_stop(env: &Env) -> Result<()> {
    match crate::jvm::runtime::STATUS.begin_stop() {
        Ok(()) => {
            // This defun runs on the main thread — the one `call_direct` may
            // have permanently attached. Detach it here (best-effort, and a
            // no-op if it was never attached) so the worker's `destroy` does
            // not block waiting for it (`destroy` waits only for *other*
            // non-daemon threads; the worker itself is the caller, exempt).
            // Reset the flag so a later restart re-attaches on its next
            // direct call.
            if let Some(vm) = crate::jvm::runtime::vm() {
                let _ = vm.detach_current_thread();
            }
            crate::jvm::runtime::set_main_attached(false);
            match crate::jvm::inbox::submit(crate::jvm::inbox::Cmd::Stop) {
                Ok(()) => Ok(()),
                Err(()) => env.signal(jvm_illegal_transition, ("no running JVM worker thread",)),
            }
        }
        Err(current) => env.signal(jvm_illegal_transition, (current.as_tag(),)),
    }
}

#[defun(mod_in_name = false, name = "jvm--load-jar")]
fn jvm_load_jar(env: &Env, path: String) -> Result<u64> {
    if crate::jvm::runtime::STATUS.get() != crate::protocol::Status::Ready {
        return env.signal(jvm_illegal_transition, (crate::jvm::runtime::STATUS.get().as_tag(),));
    }
    let id = crate::jvm::inbox::next_request_id();
    match crate::jvm::inbox::submit(crate::jvm::inbox::Cmd::LoadJar { id, path }) {
        Ok(()) => Ok(id),
        Err(()) => env.signal(jvm_illegal_transition, ("no running JVM worker thread",)),
    }
}

/// Synchronous counterpart of `jvm--load-jar`: adds PATH to the shared
/// classloader on *this* (the Emacs main) thread and returns `nil`, instead of
/// a request id. Signals `jvm-error` on failure (missing path, or a JNI error).
/// Shares the one classloader with the async `jvm--load-jar` path through a
/// single get-or-create in `runtime`, so whichever path loads the first jar
/// wins and both see every later jar (see `jvm::inbox::load_jar_direct`).
#[defun(mod_in_name = false, name = "jvm--load-jar-direct")]
fn jvm_load_jar_direct(env: &Env, path: String) -> Result<()> {
    if crate::jvm::runtime::STATUS.get() != crate::protocol::Status::Ready {
        return env.signal(jvm_illegal_transition, (crate::jvm::runtime::STATUS.get().as_tag(),));
    }
    let Some(vm) = crate::jvm::runtime::vm() else {
        return env.signal(jvm_illegal_transition, ("no running JVM",));
    };
    match crate::jvm::inbox::load_jar_direct(&vm, &path) {
        Ok(()) => Ok(()),
        Err(e) => env.signal(jvm_error, (e,)),
    }
}

#[defun(mod_in_name = false, name = "jvm--call")]
fn jvm_call(env: &Env, target: Value, member: String, args: Value) -> Result<u64> {
    if crate::jvm::runtime::STATUS.get() != crate::protocol::Status::Ready {
        return env.signal(jvm_illegal_transition, (crate::jvm::runtime::STATUS.get().as_tag(),));
    }
    let target = parse_call_target(target)?;
    let args = lisp_list_to_args(args)?;
    let id = crate::jvm::inbox::next_request_id();
    match crate::jvm::inbox::submit(crate::jvm::inbox::Cmd::Call { id, target, member, args }) {
        Ok(()) => Ok(id),
        Err(()) => env.signal(jvm_illegal_transition, ("no running JVM worker thread",)),
    }
}

/// Interprets a `jvm-call`/`jvm-call-direct` TARGET argument: a string is a
/// fully-qualified class name (for a static call or `<init>`); anything else
/// must be a `JvmHandle` `user-ptr` naming a live JVM object.
///
/// `JvmHandle` only has a `FromLisp` impl for `&JvmHandle` (the `emacs`
/// crate's blanket `user-ptr` impl is `impl<'e, T: Transfer> FromLisp<'e> for
/// &'e T`, not for a bare `T` — verified against the vendored emacs-0.21.0
/// source, `src/types/user_ptr.rs`), so this asks for a reference, not an
/// owned value.
fn parse_call_target(target: Value) -> Result<crate::protocol::CallTarget> {
    if let Ok(class_name) = target.into_rust::<String>() {
        Ok(crate::protocol::CallTarget::ClassName(class_name))
    } else {
        let h: &crate::jvm::convert::JvmHandle = target.into_rust()?;
        Ok(crate::protocol::CallTarget::Object(h.0.clone()))
    }
}

/// Synchronous counterpart of `jvm--call`: resolves and invokes
/// TARGET.MEMBER with ARGS on *this* (the Emacs main) thread, and returns the
/// result value directly instead of a request id. The main thread stays
/// permanently attached to the running JVM across calls (stable thread
/// identity + persistent thread-locals); see `jvm::invoke::call_direct`. A
/// failed call signals `jvm-error`.
///
/// Distinct from `jvm-call-sync` (`elisp/jvm.el`), which is the *async*
/// `jvm--call` plus a blocking wait on the worker's `call-done` reply. This
/// primitive never touches the worker thread or the channel: it is the
/// experimental "attach, execute, return" path (see plan
/// `docs/superpowers/plans/2026-09-13-sync-lisp-clojure-plan.md`). A long call
/// blocks Emacs's main loop for its whole duration — the caller must keep
/// heavy/blocking work on the Clojure side's own threads.
#[defun(mod_in_name = false, name = "jvm--call-direct")]
fn jvm_call_direct<'e>(
    env: &'e Env,
    target: Value,
    member: String,
    args: Value,
) -> Result<Value<'e>> {
    if crate::jvm::runtime::STATUS.get() != crate::protocol::Status::Ready {
        return env.signal(jvm_illegal_transition, (crate::jvm::runtime::STATUS.get().as_tag(),));
    }
    let Some(vm) = crate::jvm::runtime::vm() else {
        return env.signal(jvm_illegal_transition, ("no running JVM",));
    };
    let target = parse_call_target(target)?;
    let args = lisp_list_to_args(args)?;
    let loader = crate::jvm::runtime::loader();
    // Record this frame's `Env` so any Clojure -> Lisp callback made *during*
    // the call runs directly on it (`lisp::runtime`/`jvm::entry`), rather than
    // deadlocking on the async queue this blocked main thread cannot drain.
    // Dropped when the call returns or unwinds.
    let _scope = crate::lisp::runtime::enter(env);
    match crate::jvm::invoke::call_direct(&vm, &loader, &target, &member, &args) {
        Ok(arg) => crate::lisp::convert::from_bridge(env, &arg),
        // A Lisp signal/throw raised in a reentrant callback, unwound back up
        // through Clojure: rebind its symbol/data (or tag/value) to this
        // frame's `Env` and re-raise the *original* non-local exit, so the
        // round trip is transparent to the Lisp caller. `signal`/`throw` always
        // return `Err` (the non-local exit), which the defun trampoline turns
        // back into a real Emacs signal/throw.
        Err(crate::jvm::invoke::DirectError::NonLocalExit { kind, a, b }) => {
            let a = crate::lisp::convert::from_input(env, &a)?;
            let b = crate::lisp::convert::from_input(env, &b)?;
            if kind == 0 {
                env.call("signal", (a, b))
            } else {
                env.call("throw", (a, b))
            }
        }
        Err(crate::jvm::invoke::DirectError::Jni(e)) => env.signal(jvm_error, (e.to_string(),)),
    }
}

/// Drains the inbound queue (`lisp::inbox`) and runs each message's `Env`
/// op on this (the main) thread — the module-primitive half of both the
/// built-in-Lisp-access design (see design spec, "Request/reply flow") and
/// every other JVM→Emacs message (`call-done`/`load-jar-done`/`invoke`/
/// `release`/`status`/`log`). Set as the pipe process's `:filter`
/// (`jvm--ensure-pipe-process`, `elisp/jvm.el`), so Emacs calls it directly
/// as `(process output-string)`; both args are ignored — the raw wakeup
/// byte (`lisp::inbox::submit`'s poke) carries no payload, since the actual
/// message rides the Rust-side queue, not the wire.
#[defun(mod_in_name = false, name = "jvm--service-inbound")]
fn jvm_service_inbound(env: &Env, _process: Value, _data: Value) -> Result<()> {
    crate::lisp::inbox::service_inbound(env)
}

/// Replies to a pending JVM-initiated `invoke` request. VALUE is either the
/// Lisp callback's return value (OK is `t`) or the real `(SYMBOL . DATA)`
/// error cons `condition-case` caught (OK is `nil`), per `jvm--resolve-
/// invoke`'s two call sites in `elisp/jvm.el`. Marshals either shape into a
/// `LispOutcome` — a plain return value the same way an ordinary callback
/// result always has (`lisp::convert::to_bridge`), or the real signal symbol/data
/// via `lisp::convert::to_outcome` (the same classifier the Clojure
/// apply path's `error_to_reply` uses) — and sends `Cmd::InvokeReply` to the
/// worker thread, which resolves (or exceptionally completes, with a real
/// `LispSignal`) the `CompletableFuture` `jvm::entry::lisp_function_call` handed
/// back to Java — the reverse direction of `jvm--call`. Silently drops the
/// reply (like every other `jvm::inbox::submit` caller in this file) if
/// there's no running worker thread to receive it; there is nothing
/// sensible to signal back to Java in that case, since the Java side is
/// only ever blocked on its own future, not on this call succeeding.
#[defun(mod_in_name = false, name = "jvm--invoke-reply")]
fn jvm_invoke_reply(env: &Env, id: u64, ok: Value, value: Value) -> Result<()> {
    // The `emacs` crate (0.21.0) has no `FromLisp` impl for `bool` (Emacs
    // Lisp has no dedicated boolean type -- see `lisp::convert::to_bridge`'s
    // own note on this same point), so OK is received as a raw `Value` and
    // checked for Lisp truthiness directly, matching `jvm--invoke-reply`'s
    // two call sites in `elisp/jvm.el`, which always pass literally
    // `t` or `nil`.
    let outcome = if ok.is_not_nil() {
        crate::protocol::LispOutcome::Value(crate::lisp::convert::to_bridge(value)?)
    } else {
        // VALUE is the real `(SYMBOL . DATA)' condition-case caught --
        // classify each half through the same `to_outcome' the
        // Lisp-access apply path's `error_to_reply' uses, so a callback
        // signal surfaces to Java as a real `LispSignal' carrying the
        // actual symbol/data, not a fabricated `error'/stringified message.
        let symbol = value.car()?;
        let data = value.cdr()?;
        crate::protocol::LispOutcome::Signal {
            symbol: Box::new(crate::lisp::convert::to_outcome(env, symbol)?),
            data: Box::new(crate::lisp::convert::to_outcome(env, data)?),
        }
    };
    let _ = crate::jvm::inbox::submit(crate::jvm::inbox::Cmd::InvokeReply { id, outcome });
    Ok(())
}

/// Walks a Lisp list of arguments (as raw `Value`s, same pattern as the
/// existing `string_list` helper) converting each to a `BridgeValue` by
/// runtime type — see `lisp::convert::to_bridge`'s doc comment for what each
/// Lisp type maps to, and what happens to a value this bridge doesn't
/// recognize.
fn lisp_list_to_args(mut list: Value) -> Result<Vec<crate::protocol::BridgeValue>> {
    let mut out = Vec::new();
    while list.is_not_nil() {
        let item: Value = list.car()?;
        out.push(crate::lisp::convert::to_bridge(item)?);
        list = list.cdr()?;
    }
    Ok(out)
}

#[cfg(feature = "test-support")]
#[defun(mod_in_name = false, name = "jvm--test-panic")]
fn jvm_test_panic(env: &Env) -> Result<()> {
    match crate::jvm::inbox::submit(crate::jvm::inbox::Cmd::TestPanic) {
        Ok(()) => Ok(()),
        Err(()) => env.signal(jvm_illegal_transition, ("no running JVM worker thread",)),
    }
}

/// Converts a Lisp list of strings into a `Vec<String>` by walking its cons
/// cells. `emacs` 0.21 has no `FromLisp` impl for `Vec<T>` (`#[defun]`
/// parameters must be scalar `FromLisp` types, `&Env`, or `Value`/its
/// sub-types like `Vector` — verified against the compiler's own error and
/// the crate source, which lists only scalar/`Value`/`Vector`/`GlobalRef`
/// impls), so `classpath`/`options` are received as raw `Value` and
/// converted here instead of directly as `Vec<String>` parameters.
fn string_list(mut list: Value) -> Result<Vec<String>> {
    let mut out = Vec::new();
    while list.is_not_nil() {
        out.push(list.car()?);
        list = list.cdr()?;
    }
    Ok(out)
}
