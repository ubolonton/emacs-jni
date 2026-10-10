//! The module's Lisp entry points: `init`, the `jvm--*` defuns, and the `jvm-*` error symbols.
//!
//! `///` comments on a `#[defun]` become the Lisp docstring, so they follow Emacs conventions
//! (UPPERCASE arguments, `` `foo' `` quoting, 80 columns). Notes for Rust readers go in `//`
//! comments inside the body.

use emacs::{defun, Env, Result, Value};

emacs::plugin_is_GPL_compatible!();

#[emacs::module(defun_prefix = "", separator = "")]
fn init(env: &Env) -> Result<Value<'_>> {
    // Module load runs on the Emacs main thread. Only this thread attaches to the JVM
    // permanently, in `jvm--call-direct`.
    crate::jvm::runtime::set_main_thread();
    let feature = env.intern("emacs-jni")?;
    env.call("provide", (feature,))
}

/// Return t.  Use this to check that the module is loaded.
#[defun(mod_in_name = false, name = "jvm-ping")]
fn jvm_ping() -> Result<bool> {
    Ok(true)
}

emacs::define_errors! {
    jvm_illegal_transition "JVM operation not allowed in the current state"
    jvm_invalid_argument "Unsupported Lisp value for a JVM call argument"
    jvm_error "JVM call failed"
}

/// Open the JVM-to-Emacs wakeup channel on pipe PROCESS.
/// JVM threads write to it to make Emacs run `jvm--service-inbound'.
#[defun(mod_in_name = false, name = "jvm--init-channel")]
fn jvm_init_channel(env: &Env, process: Value) -> Result<()> {
    crate::lisp::inbox::init(env, process)
}

/// Start the JVM with CLASSPATH and OPTIONS, or resume a paused one.
/// CLASSPATH and OPTIONS are lists of strings.  A resume ignores them.
/// Return immediately; the new status arrives through `jvm--apply-status'.
/// Signal `jvm-illegal-transition' if the current status does not allow a start.
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

/// Detach the worker thread from the JVM, and keep the JVM.
/// Return immediately; the new status arrives through `jvm--apply-status'.
/// Signal `jvm-illegal-transition' if the JVM is not `ready'.
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

/// Destroy the JVM.
/// Return immediately; the new status arrives through `jvm--apply-status'.
/// Signal `jvm-illegal-transition' if the JVM is not `ready' or `paused'.
#[defun(mod_in_name = false, name = "jvm--stop")]
fn jvm_stop(env: &Env) -> Result<()> {
    match crate::jvm::runtime::STATUS.begin_stop() {
        Ok(()) => {
            // `call_direct` may have attached this (main) thread. Detach it, or the worker's
            // `destroy` blocks waiting for it.
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

/// Add the jar or directory at PATH to the shared classloader, on the worker.
/// Return a request id.  The result arrives through `jvm--resolve-load-jar'.
/// Signal `jvm-illegal-transition' if the JVM is not `ready'.
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

/// Add the jar or directory at PATH to the shared classloader, on this thread.
/// This is the synchronous counterpart of `jvm--load-jar'.  Both use one
/// classloader.  Signal `jvm-error' on failure.
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

/// Call TARGET.MEMBER with the list ARGS, on a new worker-side thread.
/// TARGET is a class name string or a wrapped JVM object.  Return a request
/// id.  The result arrives through `jvm--resolve-call'.  Signal
/// `jvm-illegal-transition' if the JVM is not `ready'.  See `jvm-call-async'
/// and `jvm-new-async'.
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

/// Interprets the `target` of a `jvm-call`/`jvm-call-direct`: a string is a class name (for a
/// static call or `<init>`); anything else must be a [`JvmHandle`](crate::jvm::convert::JvmHandle)
/// `user-ptr`.
fn parse_call_target(target: Value) -> Result<crate::protocol::CallTarget> {
    if let Ok(class_name) = target.into_rust::<String>() {
        Ok(crate::protocol::CallTarget::ClassName(class_name))
    } else {
        let h: &crate::jvm::convert::JvmHandle = target.into_rust()?;
        Ok(crate::protocol::CallTarget::Object(h.0.clone()))
    }
}

/// Call TARGET.MEMBER with the list ARGS on this thread, and return the result.
/// This is the synchronous counterpart of `jvm--call'.  It blocks Emacs for
/// the whole call.  `jvm-call-await' is different: it waits on the async path.
///
/// The main thread stays attached to the JVM across calls.  A Lisp signal or
/// throw from a Lisp callback inside the call propagates as itself.  Signal
/// `jvm-error' on any other failure.
#[defun(mod_in_name = false, name = "jvm--call-direct")]
fn jvm_call_direct<'e>(
    env: &'e Env,
    target: Value,
    member: String,
    args: Value,
) -> Result<Value<'e>> {
    // See `crate::jvm::invoke::call_direct` for the permanent attach.
    if crate::jvm::runtime::STATUS.get() != crate::protocol::Status::Ready {
        return env.signal(jvm_illegal_transition, (crate::jvm::runtime::STATUS.get().as_tag(),));
    }
    let Some(vm) = crate::jvm::runtime::vm() else {
        return env.signal(jvm_illegal_transition, ("no running JVM",));
    };
    let target = parse_call_target(target)?;
    let args = lisp_list_to_args(args)?;
    let loader = crate::jvm::runtime::loader();
    // A Clojure-to-Lisp callback during the call must run on this `Env`. The async queue would
    // deadlock, because this thread is the one that drains it.
    let _scope = crate::lisp::runtime::enter(env);
    match crate::jvm::invoke::call_direct(&vm, &loader, &target, &member, &args) {
        Ok(arg) => crate::lisp::convert::from_bridge(env, &arg),
        // A Lisp signal or throw from a reentrant callback. Re-raise the original non-local exit
        // so the round trip is transparent to the Lisp caller.
        Err(crate::jvm::invoke::DirectError::NonLocalExit { kind, a, b }) => {
            let a = crate::lisp::convert::from_input(env, &a)?;
            let b = crate::lisp::convert::from_input(env, &b)?;
            if kind == crate::protocol::NON_LOCAL_EXIT_SIGNAL {
                env.call("signal", (a, b))
            } else {
                env.call("throw", (a, b))
            }
        }
        Err(crate::jvm::invoke::DirectError::Jni(e)) => env.signal(jvm_error, (e.to_string(),)),
    }
}

/// Run the messages that JVM threads queued for Emacs.
/// This is the `:filter' of the wakeup pipe process.  Both arguments are
/// ignored: the wakeup byte has no payload.
#[defun(mod_in_name = false, name = "jvm--service-inbound")]
fn jvm_service_inbound(env: &Env, _process: Value, _data: Value) -> Result<()> {
    // The queue is `crate::lisp::inbox`.
    crate::lisp::inbox::service_inbound(env)
}

/// Reply to the JVM-initiated callback call ID.
/// If OK is non-nil, VALUE is the callback's return value.  Otherwise VALUE
/// is the (SYMBOL . DATA) that `condition-case' caught; it reaches Java as a
/// `LispSignal'.
#[defun(mod_in_name = false, name = "jvm--invoke-reply")]
fn jvm_invoke_reply(env: &Env, id: u64, ok: Value, value: Value) -> Result<()> {
    // If no worker is running, the reply is dropped: the Java side waits only on its own future.
    let outcome = if ok.is_not_nil() {
        crate::protocol::LispOutcome::Value(crate::lisp::convert::to_bridge(value)?)
    } else {
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

/// Converts a Lisp list of arguments to [`BridgeValue`](crate::protocol::BridgeValue)s.
fn lisp_list_to_args(mut list: Value) -> Result<Vec<crate::protocol::BridgeValue>> {
    let mut out = Vec::new();
    while list.is_not_nil() {
        let item: Value = list.car()?;
        out.push(crate::lisp::convert::to_bridge(item)?);
        list = list.cdr()?;
    }
    Ok(out)
}

/// Make the worker thread panic.  Only for crash-handling tests.
#[cfg(feature = "test-support")]
#[defun(mod_in_name = false, name = "jvm--test-panic")]
fn jvm_test_panic(env: &Env) -> Result<()> {
    match crate::jvm::inbox::submit(crate::jvm::inbox::Cmd::TestPanic) {
        Ok(()) => Ok(()),
        Err(()) => env.signal(jvm_illegal_transition, ("no running JVM worker thread",)),
    }
}

/// Converts a Lisp list of strings to a [`Vec<String>`]. `emacs` 0.21 has no
/// [`FromLisp`](emacs::FromLisp) for [`Vec<T>`].
fn string_list(mut list: Value) -> Result<Vec<String>> {
    let mut out = Vec::new();
    while list.is_not_nil() {
        out.push(list.car()?);
        list = list.cdr()?;
    }
    Ok(out)
}
