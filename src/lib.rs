use emacs::{defun, Env, Result, Value};

mod channel;
mod commands;
mod handles;
mod jvm_thread;
mod protocol;
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
