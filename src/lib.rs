use emacs::{defun, Env, Result, Value};

mod channel;
mod commands;
mod handles;
mod jvm_thread;
mod lisp_requests;
mod marshal;
mod pending;
mod protocol;
mod reentry;
mod runtime;
mod shim;
mod status;
mod worker;

emacs::plugin_is_GPL_compatible!();

#[emacs::module(defun_prefix = "", separator = "")]
fn init(env: &Env) -> Result<Value<'_>> {
    let feature = env.intern("emacs-jni")?;
    env.call("provide", (feature,))
}

#[defun(mod_in_name = false, name = "jvm-ping")]
fn jvm_ping() -> Result<bool> {
    Ok(true)
}

use crate::status::{Status, STATUS};

emacs::define_errors! {
    jvm_illegal_transition "JVM operation not allowed in the current state"
    jvm_invalid_argument "Unsupported Lisp value for a JVM call argument"
    jvm_error "JVM call failed"
}

#[defun(mod_in_name = false, name = "jvm--init-channel")]
fn jvm_init_channel(env: &Env, process: Value) -> Result<()> {
    channel::init(env, process)
}

#[defun(mod_in_name = false, name = "jvm--start")]
fn jvm_start(env: &Env, classpath: Value, options: Value) -> Result<()> {
    match STATUS.begin_start() {
        Ok(()) => {
            worker::start_or_resume(string_list(classpath)?, string_list(options)?);
            Ok(())
        }
        Err(current) => env.signal(jvm_illegal_transition, (current.as_tag(),)),
    }
}

#[defun(mod_in_name = false, name = "jvm--pause")]
fn jvm_pause(env: &Env) -> Result<()> {
    match STATUS.begin_pause() {
        Ok(()) => match worker::send_command(commands::Cmd::Pause) {
            Ok(()) => Ok(()),
            Err(()) => env.signal(jvm_illegal_transition, ("no running JVM worker thread",)),
        },
        Err(current) => env.signal(jvm_illegal_transition, (current.as_tag(),)),
    }
}

#[defun(mod_in_name = false, name = "jvm--stop")]
fn jvm_stop(env: &Env) -> Result<()> {
    match STATUS.begin_stop() {
        Ok(()) => match worker::send_command(commands::Cmd::Stop) {
            Ok(()) => Ok(()),
            Err(()) => env.signal(jvm_illegal_transition, ("no running JVM worker thread",)),
        },
        Err(current) => env.signal(jvm_illegal_transition, (current.as_tag(),)),
    }
}

#[defun(mod_in_name = false, name = "jvm--load-jar")]
fn jvm_load_jar(env: &Env, path: String) -> Result<u64> {
    if STATUS.get() != Status::Ready {
        return env.signal(jvm_illegal_transition, (STATUS.get().as_tag(),));
    }
    let id = worker::next_request_id();
    match worker::send_command(commands::Cmd::LoadJar { id, path }) {
        Ok(()) => Ok(id),
        Err(()) => env.signal(jvm_illegal_transition, ("no running JVM worker thread",)),
    }
}

#[defun(mod_in_name = false, name = "jvm--call")]
fn jvm_call(env: &Env, target: Value, member: String, args: Value) -> Result<u64> {
    if STATUS.get() != Status::Ready {
        return env.signal(jvm_illegal_transition, (STATUS.get().as_tag(),));
    }
    let target = parse_call_target(target)?;
    let args = lisp_list_to_args(args)?;
    let id = worker::next_request_id();
    match worker::send_command(commands::Cmd::Call { id, target, member, args }) {
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
fn parse_call_target(target: Value) -> Result<commands::CallTarget> {
    if let Ok(class_name) = target.into_rust::<String>() {
        Ok(commands::CallTarget::ClassName(class_name))
    } else {
        let h: &handles::JvmHandle = target.into_rust()?;
        Ok(commands::CallTarget::Object(h.0))
    }
}

/// Synchronous counterpart of `jvm--call`: resolves and invokes
/// TARGET.MEMBER with ARGS on *this* (the Emacs main) thread, and returns the
/// result value directly instead of a request id. Attaches the main thread to
/// the running JVM, runs the call, detaches when its outermost frame returns
/// (`jvm_thread::call_direct`). A failed call signals `jvm-error`.
///
/// Distinct from `jvm-call-sync` (`elisp/jvm.el`), which is the *async*
/// `jvm--call` plus a blocking wait on the worker's `call-done` reply. This
/// primitive never touches the worker thread or the channel: it is the
/// experimental "attach, execute, return" path (see plan
/// `docs/superpowers/plans/2026-09-13-sync-lisp-clojure-plan.md`). A long call
/// blocks Emacs's main loop for its whole duration — the caller must keep
/// heavy/blocking work on the Clojure side's own threads.
#[defun(mod_in_name = false, name = "jvm--call-direct")]
fn jvm_call_direct<'e>(env: &'e Env, target: Value, member: String, args: Value) -> Result<Value<'e>> {
    if STATUS.get() != Status::Ready {
        return env.signal(jvm_illegal_transition, (STATUS.get().as_tag(),));
    }
    let Some(vm) = runtime::vm() else {
        return env.signal(jvm_illegal_transition, ("no running JVM",));
    };
    let target = parse_call_target(target)?;
    let args = lisp_list_to_args(args)?;
    let loader = runtime::loader();
    // Record this frame's `Env` so any Clojure -> Lisp callback made *during*
    // the call runs directly on it (`reentry`/`shim`), rather than deadlocking
    // on the async queue this blocked main thread cannot drain. Dropped when
    // the call returns or unwinds.
    let _scope = reentry::enter(env);
    match jvm_thread::call_direct(&vm, &loader, &target, &member, &args) {
        Ok(arg) => lisp_requests::lisp_arg_to_value(env, &arg),
        Err(e) => env.signal(jvm_error, (e.to_string(),)),
    }
}

/// Constructs a `JvmHandle` `user-ptr` from a raw handle integer received
/// over the wire (in a `(jvm-handle N)` tag — see `marshal::lisp_arg_to_sexp`).
/// The Rust-side handle table entry itself already exists by the time this
/// runs (it was populated by `marshal::jvm_value_to_lisp_arg` when the
/// value first crossed from Java to Lisp); this just gives Lisp a
/// `user-ptr` wrapper for the handle number it already received.
#[defun(mod_in_name = false, name = "jvm--wrap-handle")]
fn jvm_wrap_handle(handle: u64) -> Result<Box<handles::JvmHandle>> {
    Ok(Box::new(handles::JvmHandle(handle)))
}

/// Drains the Lisp-access request queue (`lisp_requests`) and runs each
/// request's `Env` op on this (the main) thread — the module-primitive
/// half of the built-in-Lisp-access design (see design spec, "Request/
/// reply flow"). Called from `jvm--dispatch-message`'s `(lisp-request)`
/// case (`elisp/jvm.el`), itself triggered by a JVM thread's
/// `channel::send`-based wakeup after enqueuing a request (`shim.rs`'s
/// `Lisp.*`/`LispRefReleaseNotifier` natives).
#[defun(mod_in_name = false, name = "jvm--service-lisp-requests")]
fn jvm_service_lisp_requests(env: &Env) -> Result<()> {
    lisp_requests::service(env)
}

/// Replies to a pending JVM-initiated `invoke` request: RESULT is either
/// the Lisp function's return value (OK is `t`) or a formatted error
/// message (OK is `nil`), per `jvm--dispatch-message`'s `invoke` case in
/// `elisp/jvm.el`. Sends `Cmd::InvokeReply` to the worker thread,
/// which resolves (or exceptionally completes) the `CompletableFuture`
/// `shim::lisp_function_call` handed back to Java — the reverse direction
/// of `jvm--call`. Silently drops the reply (like every other `worker::
/// send_command` caller in this file) if there's no running worker thread
/// to receive it; there is nothing sensible to signal back to Java in that
/// case, since the Java side is only ever blocked on its own future, not
/// on this call succeeding.
#[defun(mod_in_name = false, name = "jvm--invoke-reply")]
fn jvm_invoke_reply(id: u64, ok: Value, result: Value) -> Result<()> {
    // The `emacs` crate (0.21.0) has no `FromLisp` impl for `bool` (Emacs
    // Lisp has no dedicated boolean type -- see `lisp_value_to_arg`'s own
    // note on this same point), so OK is received as a raw `Value` and
    // checked for Lisp truthiness directly, matching `jvm--invoke-reply`'s
    // two call sites in `elisp/jvm.el`, which always pass literally
    // `t` or `nil`.
    let ok = ok.is_not_nil();
    let arg = if ok {
        lisp_value_to_arg_no_callback(result)?
    } else {
        let message: String = result.into_rust()?;
        commands::LispArg::Str(message)
    };
    let _ = worker::send_command(commands::Cmd::InvokeReply { id, ok, result: arg });
    Ok(())
}

/// Same scalar cases as `lisp_value_to_arg` (Task 6) minus the
/// function-wrapping case — a callback's *own* return value being itself
/// another bare Lisp function is out of scope for this plan (nothing in
/// the spec's marshalling table requires it, and it would need the same
/// `env`-threading `lisp_value_to_arg` already does, for a case this plan
/// doesn't need to support yet). Values this function doesn't recognize —
/// including a bare Lisp function, since the function-wrapping case above
/// is unsupported here — signal `jvm-invalid-argument`, the same
/// "don't silently misrepresent an unmarshallable value" convention
/// `lisp_value_to_arg` itself established, rather than the older, since-
/// corrected behavior of silently defaulting to `t`.
fn lisp_value_to_arg_no_callback(v: Value) -> Result<commands::LispArg> {
    if !v.is_not_nil() {
        return Ok(commands::LispArg::Nil);
    }
    if let Ok(n) = v.into_rust::<i64>() {
        return Ok(commands::LispArg::Int(n));
    }
    // See `lisp_value_to_arg`'s matching check for why bignums must be
    // routed through `LispArg::BigInt` rather than falling through to the
    // lossy `f64` conversion below.
    if v.env.call("integerp", (v,))?.is_not_nil() {
        let digits: String = v.env.call("number-to-string", (v,))?.into_rust()?;
        return Ok(commands::LispArg::BigInt(digits));
    }
    if let Ok(f) = v.into_rust::<f64>() {
        return Ok(commands::LispArg::Float(f));
    }
    if let Ok(s) = v.into_rust::<String>() {
        return Ok(commands::LispArg::Str(s));
    }
    if let Ok(h) = v.into_rust::<&handles::JvmHandle>() {
        return Ok(commands::LispArg::Object(h.0));
    }
    if v == v.env.intern("t")? {
        return Ok(commands::LispArg::True);
    }
    v.env.signal(jvm_invalid_argument, (v,))
}

/// Walks a Lisp list of arguments (as raw `Value`s, same pattern as the
/// existing `string_list` helper) converting each to a `LispArg` by
/// runtime type — see `lisp_value_to_arg`'s doc comment for what each
/// Lisp type maps to, and what happens to a value this bridge doesn't
/// recognize.
fn lisp_list_to_args(mut list: Value) -> Result<Vec<commands::LispArg>> {
    let mut out = Vec::new();
    while list.is_not_nil() {
        let item: Value = list.car()?;
        out.push(lisp_value_to_arg(item)?);
        list = list.cdr()?;
    }
    Ok(out)
}

/// Converts a single Lisp value to a `LispArg` by runtime type, trying
/// each marshallable Lisp type in turn.
fn lisp_value_to_arg(v: Value) -> Result<commands::LispArg> {
    if !v.is_not_nil() {
        return Ok(commands::LispArg::Nil);
    }
    if let Ok(n) = v.into_rust::<i64>() {
        return Ok(commands::LispArg::Int(n));
    }
    // A Lisp integer (fixnum or bignum) that didn't fit in i64 above is a
    // bignum too large for a 64-bit register — this must be routed
    // through `LispArg::BigInt` via its exact decimal string, not the
    // `f64` attempt below. `extract_float` (which backs `into_rust::<f64>`)
    // coerces any Lisp number, including bignums, so skipping this check
    // would silently accept a bignum as a lossy `LispArg::Float` instead of
    // erroring or round-tripping exactly — the same "don't silently
    // misrepresent" concern `marshal::jvm_value_to_lisp_arg` (Task 4)
    // already applies on the Java-to-Lisp direction for boxed Long/Integer.
    if v.env.call("integerp", (v,))?.is_not_nil() {
        let digits: String = v.env.call("number-to-string", (v,))?.into_rust()?;
        return Ok(commands::LispArg::BigInt(digits));
    }
    if let Ok(f) = v.into_rust::<f64>() {
        return Ok(commands::LispArg::Float(f));
    }
    if let Ok(s) = v.into_rust::<String>() {
        return Ok(commands::LispArg::Str(s));
    }
    if let Ok(h) = v.into_rust::<&handles::JvmHandle>() {
        return Ok(commands::LispArg::Object(h.0));
    }
    // `t` has no useful into_rust::<T>() scalar conversion (there's no
    // `FromLisp` impl for `bool` — Emacs Lisp has no dedicated boolean
    // type), so it's checked explicitly here via `eq` against the
    // interned `t` symbol, matching Lisp's own notion of "is this
    // literally `t`" rather than "is this any non-nil symbol".
    if v == v.env.intern("t")? {
        return Ok(commands::LispArg::True);
    }
    // A Lisp function (symbol with a function cell, lambda, closure,
    // subr, or byte-compiled object) is registered Lisp-side in
    // `jvm--lisp-callbacks' rather than tracked here — this module never
    // holds a reference to the function itself, only its handle. See
    // spec "Identity and lifetime": Lisp owns that table, since Rust has
    // no stable way to hold onto an arbitrary Lisp function value across
    // calls. `marshal::lisp_arg_to_jvalue` later turns this handle into
    // a real `LispFunction' proxy object when the argument actually
    // crosses into Java.
    //
    // The `emacs` crate (0.21.0, checked against its vendored source —
    // `env.rs`/`value.rs`/`call.rs`) has no `Value::is_function` or
    // similar native predicate, so this calls Lisp's own `functionp'
    // via `env.call`, the same "call back into Lisp for a predicate the
    // crate doesn't expose" pattern already used above for `integerp'.
    if v.env.call("functionp", (v,))?.is_not_nil() {
        let handle: u64 = v.env.call("jvm--register-callback", (v,))?.into_rust()?;
        return Ok(commands::LispArg::Callback(handle));
    }
    // Anything else reaching this point (a vector, keyword, hash-table,
    // or any symbol other than `t`) is not a value this bridge knows how
    // to marshal. Earlier code silently treated it as `LispArg::True`,
    // which could mask real caller bugs (e.g. accidentally passing a
    // vector where a number was intended would silently become Java
    // `true`) -- signal a clear error instead.
    v.env.signal(jvm_invalid_argument, (v,))
}

#[cfg(feature = "test-support")]
#[defun(mod_in_name = false, name = "jvm--test-panic")]
fn jvm_test_panic(env: &Env) -> Result<()> {
    match worker::send_command(commands::Cmd::TestPanic) {
        Ok(()) => Ok(()),
        Err(()) => env.signal(jvm_illegal_transition, ("no running JVM worker thread",)),
    }
}

/// Throwaway plumbing-only defun: proves a `handles::JvmHandle` round-trips
/// through Lisp as a genuine `user-ptr` before Task 5 replaces this with the
/// real `jvm-call`/`jvm-new` machinery that produces `JvmHandle`s from
/// actual method calls. Returning `Box<handles::JvmHandle>` (rather than a
/// bare `handles::JvmHandle`) is what the `emacs` crate's blanket
/// `impl<T: Transfer> IntoLisp for Box<T>` picks up to embed the value as a
/// `user-ptr` -- see `src/handles.rs`'s `Transfer` impl.
#[cfg(feature = "test-support")]
#[defun(mod_in_name = false, name = "jvm--test-make-handle")]
fn jvm_test_make_handle() -> Result<Box<handles::JvmHandle>> {
    // A handle registered against nothing real (no live JVM object needed
    // for this plumbing-only test) — Task 5 is what makes handles wrap
    // genuine JNI objects.
    Ok(Box::new(handles::JvmHandle(handles::register_dummy_for_test())))
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
