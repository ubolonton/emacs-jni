use std::sync::Arc;

use jni::vm::JavaVM;

use crate::channel;
use crate::jvm::convert::marshal_error;

/// Builds the actual Java value a successful `LispOutcome` becomes —
/// factored out of `handle_lisp_reply` since the `LispOutcome::Signal`/
/// `Throw` cases (handled by that function's caller) never reach here.
///
/// `pub(crate)` because the reentrant direct path (`jvm::entry`) builds the same
/// Java object on its own thread, from the `LispOutcome`
/// `lisp_requests::compute_reply` returns, instead of over the channel.
pub(crate) fn build_lisp_reply_object<'local>(
    env: &mut jni::Env<'local>,
    value: crate::protocol::LispOutcome,
) -> jni::errors::Result<jni::objects::JObject<'local>> {
    match value {
        crate::protocol::LispOutcome::Value(bridge) => {
            // Reuse the one Lisp->JVM marshaller against java.lang.Object,
            // then box any primitive — identical to how handle_invoke_reply
            // completes a callback's return value.
            let object_class = env.find_class(jni::jni_str!("java/lang/Object"))?;
            let jvalue = crate::jvm::convert::from_bridge(env, &bridge, &object_class)?;
            crate::jvm::convert::box_if_primitive(env, jvalue)
        }
        crate::protocol::LispOutcome::Bool(b) => {
            let class = env.find_class(jni::jni_str!("java/lang/Boolean"))?;
            env.call_static_method(
                &class,
                jni::jni_str!("valueOf"),
                jni::jni_sig!((v: boolean) -> java.lang.Boolean),
                &[jni::objects::JValue::Bool(b)],
            )?
            .l()
        }
        crate::protocol::LispOutcome::Ptr(ptr) => {
            let class = env.find_class(jni::jni_str!("emacs_jni/core/LispObject"))?;
            env.new_object(
                &class,
                jni::jni_sig!((ptr: long) -> void),
                &[jni::objects::JValue::Long(ptr)],
            )
        }
        crate::protocol::LispOutcome::Signal { .. }
        | crate::protocol::LispOutcome::Throw { .. } => {
            unreachable!(
                "Signal/Throw are handled by complete_future_with_reply/throw_non_local_exit \
                 before build_lisp_reply_object"
            )
        }
    }
}

/// Turns one `LispOutcome` into a Java result: a `Signal`/`Throw`
/// completes FUTURE exceptionally with the matching `LispNonLocalExit`, and
/// anything else completes it normally with its built object.
///
/// Shared by the async worker (`handle_lisp_reply`, completing a registered
/// future) and the reentrant direct path (`jvm::entry`'s `applyAsync` fast path,
/// completing a fresh, immediately-returned future) — Clojure's `apply`
/// derefs that future either way, so both must produce the identical shape.
pub(crate) fn complete_future_with_reply<'local>(
    env: &mut jni::Env<'local>,
    future: &jni::objects::JObject<'local>,
    value: crate::protocol::LispOutcome,
) -> jni::errors::Result<()> {
    match value {
        crate::protocol::LispOutcome::Signal { symbol, data } => {
            let ex = build_non_local_exit(env, 0, *symbol, *data)?;
            env.call_method(
                future,
                jni::jni_str!("completeExceptionally"),
                jni::jni_sig!((ex: java.lang.Throwable) -> boolean),
                &[jni::objects::JValueOwned::Object(ex).borrow()],
            )?;
        }
        crate::protocol::LispOutcome::Throw { tag, value } => {
            let ex = build_non_local_exit(env, 1, *tag, *value)?;
            env.call_method(
                future,
                jni::jni_str!("completeExceptionally"),
                jni::jni_sig!((ex: java.lang.Throwable) -> boolean),
                &[jni::objects::JValueOwned::Object(ex).borrow()],
            )?;
        }
        other => {
            let obj = build_lisp_reply_object(env, other)?;
            env.call_method(
                future,
                jni::jni_str!("complete"),
                jni::jni_sig!((v: java.lang.Object) -> boolean),
                &[jni::objects::JValueOwned::Object(obj).borrow()],
            )?;
        }
    }
    Ok(())
}

/// Builds an `emacs_jni.core.LispSignal(a, b)` (`kind == 0`) or
/// `LispThrow(a, b)` (`kind == 1`) from two already-classified reply
/// values — both still a `LispNonLocalExit`/`Throwable`, so the existing
/// catch/rethrow sites (`describe_caught_java_exception`,
/// `extract_non_local_exit`) keep matching by the shared base.
fn build_non_local_exit<'local>(
    env: &mut jni::Env<'local>,
    kind: i32,
    a: crate::protocol::LispOutcome,
    b: crate::protocol::LispOutcome,
) -> jni::errors::Result<jni::objects::JObject<'local>> {
    let a_obj = build_lisp_reply_object(env, a)?;
    let b_obj = build_lisp_reply_object(env, b)?;
    let class = if kind == 0 {
        env.find_class(jni::jni_str!("emacs_jni/core/LispSignal"))?
    } else {
        env.find_class(jni::jni_str!("emacs_jni/core/LispThrow"))?
    };
    env.new_object(
        &class,
        jni::jni_sig!((a: java.lang.Object, b: java.lang.Object) -> void),
        &[jni::objects::JValue::Object(&a_obj), jni::objects::JValue::Object(&b_obj)],
    )
}

/// Throws a `LispNonLocalExit` for a Lisp signal/throw raised in a reentrant
/// direct-path native (`jvm::entry`'s synchronous natives), so it propagates up
/// the Clojure stack exactly like the checked exception `block_on_lisp_reply`
/// rethrows on the async path. Returns the `Error::JavaException` the caller
/// hands back so the `native_method!` policy leaves this pending exception in
/// place (rather than throwing a fresh `RuntimeException` over it).
pub(crate) fn throw_non_local_exit(
    env: &mut jni::Env,
    kind: i32,
    a: crate::protocol::LispOutcome,
    b: crate::protocol::LispOutcome,
) -> jni::errors::Error {
    let thrown = build_non_local_exit(env, kind, a, b)
        .and_then(|ex| env.cast_local::<jni::objects::JThrowable>(ex))
        .and_then(|ex| env.throw(&ex));
    match thrown {
        Err(e) => e,
        Ok(()) => marshal_error("env.throw unexpectedly left no pending exception"),
    }
}

/// Handles a `Cmd::Call`: resolves TARGET.MEMBER reflectively and invokes
/// it with ARGS, then enqueues an `Inbound::CallDone` with the result. Runs
/// on its own short-lived thread (spawned by `jvm::inbox::run`'s `Cmd::Call`
/// arm) — never inline on the worker's dispatch loop, per spec "Worker
/// thread role": an arbitrary Java call can take a long time for reasons
/// unrelated to callbacks, and must not stall other pending commands.
pub(crate) fn handle_call(
    vm: &JavaVM,
    loader: Option<Arc<jni::refs::Global<jni::objects::JObject<'static>>>>,
    id: u64,
    target: crate::protocol::CallTarget,
    member: String,
    args: Vec<crate::protocol::BridgeValue>,
) {
    let result: jni::errors::Result<crate::protocol::BridgeValue> =
        vm.attach_current_thread(|env| resolve_and_invoke(env, &loader, &target, &member, &args));

    match result {
        Ok(value) => {
            crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::CallDone {
                id,
                ok: true,
                result: value,
                err: None,
            });
        }
        Err(e) => {
            crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::CallDone {
                id,
                ok: false,
                result: crate::protocol::BridgeValue::Nil,
                err: Some(e.to_string()),
            });
        }
    }
    channel::poke();
}

/// Resolves TARGET.MEMBER against the shared classloader and invokes it with
/// ARGS on an already-attached thread, returning the marshalled result.
///
/// Shared by the async worker (`handle_call`, on a spawned thread) and the
/// synchronous main-thread path (`call_direct`). Identical behaviour in both:
/// the only differences are which thread runs it and what the caller does
/// with the `BridgeValue` afterward (an `Inbound::CallDone` enqueue vs. a
/// direct `Value`).
pub(crate) fn resolve_and_invoke(
    env: &mut jni::Env,
    loader: &Option<crate::jvm::runtime::SharedLoader>,
    target: &crate::protocol::CallTarget,
    member: &str,
    args: &[crate::protocol::BridgeValue],
) -> jni::errors::Result<crate::protocol::BridgeValue> {
    // A freshly attached thread's context classloader is null. Plain
    // JNI class resolution (find_class/loadClass, used just below)
    // doesn't need it, but Clojure's own namespace loader
    // (clojure.lang.RT, used internally by `require`) does: it loads
    // .clj source files as classpath *resources*, not classes, and
    // that resolution path falls back through
    // Thread.currentThread().getContextClassLoader(). Left null, the
    // first touch of Clojure's runtime throws
    // ExceptionInInitializerError before clojure.core even finishes
    // loading — verified empirically during Phase 3's design spike
    // (see docs/superpowers/specs/2026-09-02-clojure-nrepl-design.md,
    // "Feasibility"). Harmless for calls that never touch Clojure.
    if let Some(global) = loader {
        let loader_local = env.new_local_ref(global.as_obj())?;
        let thread_class = env.find_class(jni::jni_str!("java/lang/Thread"))?;
        let current_thread = env
            .call_static_method(
                &thread_class,
                jni::jni_str!("currentThread"),
                jni::jni_sig!(() -> java.lang.Thread),
                &[],
            )?
            .l()?;
        env.call_method(
            &current_thread,
            jni::jni_str!("setContextClassLoader"),
            jni::jni_sig!((cl: java.lang.ClassLoader) -> void),
            &[jni::objects::JValue::Object(&loader_local)],
        )?;
    }

    let (class, instance): (jni::objects::JClass, Option<jni::objects::JObject>) = match target {
        crate::protocol::CallTarget::ClassName(name) => {
            let class = match loader {
                // A jar has been loaded at least once: classes it
                // provides only live in *this* classloader's URL list,
                // not on the default `FindClass` search path (verified
                // empirically in Task 7 — see `add_jar_url`'s own test,
                // which resolves `Greeter` the same way, via
                // `loadClass`, not `find_class`). Routing every
                // class-name lookup through the shared loader once it
                // exists still finds plain JDK classes too, since
                // `URLClassLoader#loadClass` delegates to its parent
                // (the platform loader) before consulting its own URLs.
                Some(global) => {
                    let loader_local = env.new_local_ref(global.as_obj())?;
                    resolve_class_via_loader(env, &loader_local, name)?
                }
                // No jar ever loaded: fall back to a plain `FindClass`,
                // which covers plain JDK classes (e.g. "java.lang.Math")
                // without needing a classloader object at all.
                None => {
                    let internal_name = name.replace('.', "/");
                    env.find_class(jni::strings::JNIString::from(internal_name))?
                }
            };
            (class, None)
        }
        crate::protocol::CallTarget::Object(object) => {
            let local = env.new_local_ref(object.0.as_obj())?;
            (env.get_object_class(&local)?, Some(local))
        }
    };

    if member == "<init>" {
        invoke_constructor(env, &class, args)
    } else {
        invoke_method(env, &class, instance.as_ref(), member, args)
    }
}

thread_local! {
    /// Nesting depth of in-flight `call_direct` frames on this thread. Used
    /// only to gate the reentrant-boundary outbound path
    /// (`describe_caught_java_exception` classifies a `LispNonLocalExit` into
    /// `PENDING_NLE` only when `> 0`, i.e. inside a direct call — never on the
    /// async worker thread). It no longer drives detaching: the main thread
    /// stays permanently attached, and non-main callers rely on the scoped
    /// attach in `call_direct` to detach on return.
    static ATTACH_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };

    /// Side channel from `describe_caught_java_exception` back to
    /// `call_direct`, for the outbound half of the reentrant boundary: a Lisp
    /// signal/throw raised in a callback unwinds up through Clojure as a
    /// `LispNonLocalExit`; when it reaches the top of a direct call
    /// (`ATTACH_DEPTH > 0`) `describe` classifies its fields into this slot so
    /// `call_direct` can re-raise the *original* signal/throw instead of a
    /// generic `jvm-error`. A plain error can't carry them — `describe`'s
    /// return type is fixed `jni::errors::Error`. Filled at the invoke error
    /// site, taken (once) by the enclosing `call_direct`. Never touched on the
    /// async worker thread (its `ATTACH_DEPTH` stays 0), so `handle_call` is
    /// unchanged.
    static PENDING_NLE: std::cell::RefCell<Option<DirectError>> =
        const { std::cell::RefCell::new(None) };
}

/// Failure of a `call_direct`.
pub(crate) enum DirectError {
    /// A Lisp non-local exit that unwound up through Clojure as a
    /// `LispNonLocalExit`. `kind` is 0 (signal) or 1 (throw); `a`/`b` are the
    /// already-classified symbol/data or tag/value, ready to rebind to the
    /// caller's `Env` and re-raise via `(signal …)`/`(throw …)`.
    NonLocalExit { kind: i32, a: crate::protocol::LispInput, b: crate::protocol::LispInput },
    /// Any other JVM-side failure — surfaced as `jvm-error` with this message.
    Jni(jni::errors::Error),
}

/// Runs a synchronous TARGET.MEMBER call on the current (Emacs main) thread:
/// attach to the shared JVM, resolve + invoke via `resolve_and_invoke`,
/// return the marshalled result. The main thread stays permanently attached
/// (for a stable thread identity); the per-call attach is scoped, so on any
/// *other* thread it detaches on return (see the attach comments in the body).
/// Called by the `jvm--call-direct` defun, which converts the `BridgeValue` to a
/// real `Value` on the same thread's live `Env`.
pub(crate) fn call_direct(
    vm: &JavaVM,
    loader: &Option<crate::jvm::runtime::SharedLoader>,
    target: &crate::protocol::CallTarget,
    member: &str,
    args: &[crate::protocol::BridgeValue],
) -> Result<crate::protocol::BridgeValue, DirectError> {
    ATTACH_DEPTH.with(|d| d.set(d.get() + 1));
    // Clear any stale slot before this frame's invoke, so a `Some` seen after
    // it can only be this frame's (each frame fills and takes its own).
    PENDING_NLE.with(|s| *s.borrow_mut() = None);

    // Attach the Emacs main thread to the JVM *permanently*, once. A permanent
    // attach installs a thread-local guard that keeps the thread attached
    // until it terminates (or `jvm--stop` detaches it), so its
    // `java.lang.Thread` peer — hence `Thread.currentThread()` identity and any
    // thread-locals (Clojure dynamic bindings, `*ns*`) — stays stable across
    // calls. The scoped attach below is then a cheap no-op on it (it does not
    // own the attachment, so it never detaches on return). Only the main
    // thread gets this: a direct call from any other thread (e.g. a
    // `make-thread` Lisp thread) is never recorded as main, so its scoped
    // attach owns the attachment and detaches on return — transient, no leaked
    // attachment to block a later `destroy`.
    if crate::jvm::runtime::on_main_thread()
        && !crate::jvm::runtime::main_attached()
        && vm.attach_current_thread(|_env| Ok::<(), jni::errors::Error>(())).is_ok()
    {
        crate::jvm::runtime::set_main_attached(true);
    }

    // The actual invoke always goes through a *scoped* attach: a no-op on the
    // permanently-attached main thread, an owning attach-and-detach on any
    // other thread.
    let result = vm.attach_current_thread_for_scope(|env| {
        resolve_and_invoke(env, loader, target, member, args)
    });

    ATTACH_DEPTH.with(|d| d.set(d.get() - 1));
    match result {
        Ok(arg) => Ok(arg),
        // A `LispNonLocalExit` reconstructed by `describe_caught_java_exception`
        // takes precedence over its flattened message: re-raise the original
        // signal/throw. Otherwise it's a genuine JVM failure → `jvm-error`.
        Err(e) => Err(PENDING_NLE.with(|s| s.borrow_mut().take()).unwrap_or(DirectError::Jni(e))),
    }
}

/// Resolves NAME (a dotted binary class name, e.g. `"java.lang.String"` or
/// `"Calc"`) to a `Class` object via LOADER's public `loadClass` method —
/// the same approach `add_jar_url`'s own test already uses to prove a
/// jar-loaded class is reachable through this loader.
fn resolve_class_via_loader<'local>(
    env: &mut jni::Env<'local>,
    loader: &jni::objects::JObject<'_>,
    name: &str,
) -> jni::errors::Result<jni::objects::JClass<'local>> {
    let jname = env.new_string(name)?;
    let class_obj = env
        .call_method(
            loader,
            jni::jni_str!("loadClass"),
            jni::jni_sig!((name: java.lang.String) -> java.lang.Class),
            &[jni::objects::JValue::Object(&jname.into())],
        )?
        .l()?;
    env.cast_local::<jni::objects::JClass>(class_obj)
}

/// Renders an already-caught Java exception (the caller must `exception_catch()`
/// it first — see below) as a readable message that includes the whole
/// `getCause()` chain, not just the outermost exception.
///
/// This matters specifically for `Method.invoke`/`Constructor.newInstance`:
/// both always wrap whatever the *target* code actually threw in an
/// `InvocationTargetException`, whose own `getMessage()` is normally
/// `null` and whose own stack trace is just the two reflection frames
/// where it was constructed — the actually useful information (e.g. a
/// `LispNonLocalExit`'s message, for Task 7's callback round trip: `Calc.
/// callWithArg`'s `future.get(...)` throws `ExecutionException`, itself
/// wrapping the `LispNonLocalExit` `handle_invoke_reply` built) is one or more
/// `getCause()` hops further down. Left to `JavaVM::attach_current_thread`'s
/// own default exception handling (which auto-converts a still-pending
/// exception into `Error::CaughtJavaException` using exactly that shallow
/// info, once the whole `attach_current_thread` closure returns without
/// having cleared it), the caller only ever sees something like `"<NULL>
/// \nStack trace:\n0: ...DirectMethodHandleAccessor.invoke...\n1: ...
/// Method.invoke...\n"` — true, but useless for diagnosing what actually
/// went wrong (found empirically running this task's own ERT test: see
/// task-7-report.md). Describing the exception here, immediately after the
/// failing reflective call and before it can escape this function, lets the
/// message include the real cause.
///
/// Takes an already-caught exception rather than catching it itself: callers
/// at the invoke sites (`invoke_method`/`invoke_constructor`) need to inspect
/// the caught exception first (`is_overload_arg_mismatch`) to decide whether
/// to fall through to the next overload candidate or describe-and-return, so
/// the catch has to happen before this is called.
fn describe_caught_java_exception(
    env: &mut jni::Env,
    caught: jni::errors::Error,
) -> jni::errors::Error {
    let jni::errors::Error::CaughtJavaException { name, msg, exception, .. } = caught else {
        // `exception_catch` itself failed in some unexpected way (e.g.
        // couldn't query the throwable's class/message) -- surface that
        // failure as-is rather than losing it.
        return caught;
    };

    // Outbound half of the reentrant boundary: only inside a direct call
    // (`ATTACH_DEPTH > 0`, i.e. never on the async worker thread). If this
    // exception is, or wraps, a `LispNonLocalExit`, stash its classified
    // fields so the enclosing `call_direct` re-raises the original Lisp
    // signal/throw. Best-effort — any failure here just falls through to the
    // flattened message below (a `jvm-error`).
    if ATTACH_DEPTH.with(|d| d.get()) > 0 {
        if let Ok(local) = env.new_local_ref(exception.as_obj()) {
            if let Ok(throwable) = env.cast_local::<jni::objects::JThrowable>(local) {
                if let Ok(Some(nle)) = extract_non_local_exit(env, throwable) {
                    PENDING_NLE.with(|s| *s.borrow_mut() = Some(nle));
                }
            }
        }
    }

    let mut lines = vec![format!("{name}: {msg}")];
    if let Ok(local) = env.new_local_ref(exception.as_obj()) {
        if let Ok(mut current) = env.cast_local::<jni::objects::JThrowable>(local) {
            // Bounded, not unconditional: ordinary `Throwable` construction
            // can't produce a `getCause()` cycle (`initCause` rejects
            // `cause == this`), but nothing guarantees some hostile/
            // reflective subclass can't -- this is diagnostic code, not
            // worth an unbounded loop over.
            for _ in 0..10 {
                let cause = match current.get_cause(env) {
                    Ok(c) => c,
                    Err(_) => break,
                };
                if cause.is_null() {
                    break;
                }
                let cause_name = env
                    .get_object_class(&cause)
                    .and_then(|c| c.get_name(env))
                    .map(|s| s.to_string())
                    .unwrap_or_else(|_| "<unknown>".to_string());
                let cause_msg = cause
                    .get_message(env)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|_| "<NULL>".to_string());
                lines.push(format!("Caused by: {cause_name}: {cause_msg}"));
                current = cause;
            }
        }
    }
    marshal_error(&lines.join("\n"))
}

/// True when CAUGHT is a bare `IllegalArgumentException` straight off
/// `Method.invoke`/`Constructor.newInstance` — the JDK's own signal that ARGS
/// didn't fit the candidate's declared parameter types (e.g. a `Str` argument
/// marshalled fine as a `String` but the candidate actually wanted a `long`).
/// A real exception thrown by the invoked method/constructor body always
/// arrives wrapped in `InvocationTargetException` instead, so this can't
/// mistake a genuine `IllegalArgumentException` thrown *by user code* for an
/// overload mismatch — that one comes through with a different `name`.
fn is_overload_arg_mismatch(caught: &jni::errors::Error) -> bool {
    matches!(
        caught,
        jni::errors::Error::CaughtJavaException { name, .. }
            if name == "java.lang.IllegalArgumentException"
    )
}

/// Walks THROWABLE's `getCause()` chain for an `emacs_jni.core.LispNonLocalExit`
/// and, if found, classifies its `kind`/`symbolOrTag`/`dataOrValue` back into a
/// `DirectError::NonLocalExit`. The two fields run through the same
/// `jvm::convert::to_input` classifier the inbound direction built
/// them with (`build_lisp_reply_object`), so the round trip is symmetric.
/// `None` if no `LispNonLocalExit` is in the chain (a genuine Java/Clojure
/// exception).
fn extract_non_local_exit<'local>(
    env: &mut jni::Env<'local>,
    throwable: jni::objects::JThrowable<'local>,
) -> jni::errors::Result<Option<DirectError>> {
    let nle_class = env.find_class(jni::jni_str!("emacs_jni/core/LispNonLocalExit"))?;
    let mut current = throwable;
    // Bounded like `describe_caught_java_exception`'s own walk: a normal cause
    // chain can't cycle, but this is recovery code, not worth an unbounded loop.
    for _ in 0..10 {
        if current.is_null() {
            break;
        }
        if env.is_instance_of(&current, &nle_class)? {
            let kind = env.get_field(&current, jni::jni_str!("kind"), jni::jni_sig!(int))?.i()?;
            let a_obj = env
                .get_field(&current, jni::jni_str!("symbolOrTag"), jni::jni_sig!(java.lang.Object))?
                .l()?;
            let b_obj = env
                .get_field(&current, jni::jni_str!("dataOrValue"), jni::jni_sig!(java.lang.Object))?
                .l()?;
            let a = crate::jvm::convert::to_input(env, a_obj)?;
            let b = crate::jvm::convert::to_input(env, b_obj)?;
            return Ok(Some(DirectError::NonLocalExit { kind, a, b }));
        }
        current = current.get_cause(env)?;
    }
    Ok(None)
}

/// Finds an overload of METHOD_NAME on CLASS whose parameter count matches
/// ARGS, invokes it, and marshals the result. Ambiguous overloads (same
/// name, same arg count, multiple candidates) are tried in whatever order
/// `getMethods()` returns them: if marshalling ARGS against a candidate's
/// declared parameter types fails (e.g. `foo(int)` and `foo(String)` both
/// match on name and arity, but the actual argument is a string), that
/// candidate is skipped and the next same-name/arity candidate is tried,
/// rather than giving up immediately — see Finding 2 of the Phase 2
/// whole-branch review. This is a fall-through, not a real overload-
/// resolution algorithm (no assignability-based scoring of which candidate
/// is the *best* fit) — acceptable for a prototype; a real algorithm is
/// future work if this proves too coarse in practice.
fn invoke_method(
    env: &mut jni::Env<'_>,
    class: &jni::objects::JClass,
    instance: Option<&jni::objects::JObject>,
    method_name: &str,
    args: &[crate::protocol::BridgeValue],
) -> jni::errors::Result<crate::protocol::BridgeValue> {
    let methods = env
        .call_method(
            class,
            jni::jni_str!("getMethods"),
            jni::jni_sig!(() -> [java.lang.reflect.Method]),
            &[],
        )?
        .l()?;
    let methods = env.cast_local::<jni::objects::JObjectArray>(methods)?;
    let count = methods.len(env)?;

    let null_receiver = jni::objects::JObject::null();
    let receiver = instance.unwrap_or(&null_receiver);

    // Remembers the most recent invoke-time argument mismatch (a bare
    // `IllegalArgumentException`, per `is_overload_arg_mismatch`) so the
    // final "no method found" error can describe *why* the closest same-
    // name/arity candidate didn't work, instead of a bare "not found".
    let mut last_arg_mismatch: Option<jni::errors::Error> = None;

    for i in 0..count {
        let candidate = methods.get_element(env, i)?;
        let name = env
            .call_method(
                &candidate,
                jni::jni_str!("getName"),
                jni::jni_sig!(() -> java.lang.String),
                &[],
            )?
            .l()?;
        let name = env.cast_local::<jni::objects::JString>(name)?.to_string();
        if name != method_name {
            continue;
        }
        let param_types = env
            .call_method(
                &candidate,
                jni::jni_str!("getParameterTypes"),
                jni::jni_sig!(() -> [java.lang.Class]),
                &[],
            )?
            .l()?;
        let param_types = env.cast_local::<jni::objects::JObjectArray>(param_types)?;
        if param_types.len(env)? != args.len() {
            continue;
        }

        // A same-name/arity candidate whose declared parameter types ARGS
        // doesn't actually fit (e.g. `foo(int)` vs. `foo(String)`) isn't a
        // hard failure yet — another overload further down `getMethods()`'s
        // list might still match. Move on to it instead of giving up here;
        // only exhausting every same-name/arity candidate (the loop falling
        // through below) is a real "no method found".
        let invoke_args = match build_invoke_args(env, &param_types, args) {
            Ok(a) => a,
            Err(_) => continue,
        };

        let result = match env.call_method(
            &candidate,
            jni::jni_str!("invoke"),
            jni::jni_sig!((obj: java.lang.Object, args: [java.lang.Object]) -> java.lang.Object),
            &[jni::objects::JValue::Object(receiver), jni::objects::JValue::Object(&invoke_args)],
        ) {
            Ok(v) => v.l()?,
            // A bare `IllegalArgumentException` here means ARGS didn't fit
            // THIS candidate's declared parameter types (marshalling alone
            // can't catch every mismatch, e.g. `Str` fits any reference
            // parameter type) — fall through to the next same-name/arity
            // candidate instead of giving up. Anything else is a real
            // failure (or the invoked method's own exception, always
            // wrapped in `InvocationTargetException` — see
            // `describe_caught_java_exception`'s doc comment): describe and
            // return it.
            Err(jni::errors::Error::JavaException) => match env.exception_catch() {
                Ok(()) => {
                    return Err(marshal_error(
                        "expected a pending Java exception after Method.invoke but found none",
                    ))
                }
                Err(caught) if is_overload_arg_mismatch(&caught) => {
                    last_arg_mismatch = Some(caught);
                    continue;
                }
                Err(caught) => return Err(describe_caught_java_exception(env, caught)),
            },
            Err(e) => return Err(e),
        };
        return crate::jvm::convert::to_bridge(env, jni::objects::JValueOwned::Object(result));
    }

    Err(last_arg_mismatch.map_or_else(
        || marshal_error(&format!("no method {method_name} with {} argument(s) found", args.len())),
        |caught| describe_caught_java_exception(env, caught),
    ))
}

fn invoke_constructor(
    env: &mut jni::Env<'_>,
    class: &jni::objects::JClass,
    args: &[crate::protocol::BridgeValue],
) -> jni::errors::Result<crate::protocol::BridgeValue> {
    let constructors = env
        .call_method(
            class,
            jni::jni_str!("getConstructors"),
            jni::jni_sig!(() -> [java.lang.reflect.Constructor]),
            &[],
        )?
        .l()?;
    let constructors = env.cast_local::<jni::objects::JObjectArray>(constructors)?;
    let count = constructors.len(env)?;

    // See `invoke_method`'s matching field: remembers the most recent
    // invoke-time argument mismatch so the final "no constructor found"
    // error can describe why, instead of a bare "not found".
    let mut last_arg_mismatch: Option<jni::errors::Error> = None;

    for i in 0..count {
        let candidate = constructors.get_element(env, i)?;
        let param_types = env
            .call_method(
                &candidate,
                jni::jni_str!("getParameterTypes"),
                jni::jni_sig!(() -> [java.lang.Class]),
                &[],
            )?
            .l()?;
        let param_types = env.cast_local::<jni::objects::JObjectArray>(param_types)?;
        if param_types.len(env)? != args.len() {
            continue;
        }

        // See `invoke_method`'s matching comment: try the next same-arity
        // constructor overload rather than giving up on the first one whose
        // declared parameter types ARGS doesn't actually fit.
        let invoke_args = match build_invoke_args(env, &param_types, args) {
            Ok(a) => a,
            Err(_) => continue,
        };

        let instance = match env.call_method(
            &candidate,
            jni::jni_str!("newInstance"),
            jni::jni_sig!((args: [java.lang.Object]) -> java.lang.Object),
            &[jni::objects::JValue::Object(&invoke_args)],
        ) {
            Ok(v) => v.l()?,
            // See `invoke_method`'s matching arm: a bare
            // `IllegalArgumentException` here means ARGS didn't fit THIS
            // candidate's declared parameter types, so fall through to the
            // next same-arity constructor instead of giving up.
            Err(jni::errors::Error::JavaException) => match env.exception_catch() {
                Ok(()) => {
                    return Err(marshal_error(
                        "expected a pending Java exception after Constructor.newInstance but found none",
                    ))
                }
                Err(caught) if is_overload_arg_mismatch(&caught) => {
                    last_arg_mismatch = Some(caught);
                    continue;
                }
                Err(caught) => return Err(describe_caught_java_exception(env, caught)),
            },
            Err(e) => return Err(e),
        };
        return crate::jvm::convert::to_bridge(env, jni::objects::JValueOwned::Object(instance));
    }

    Err(last_arg_mismatch.map_or_else(
        || marshal_error(&format!("no constructor with {} argument(s) found", args.len())),
        |caught| describe_caught_java_exception(env, caught),
    ))
}

/// Shared by `invoke_method`/`invoke_constructor`: marshals ARGS against
/// PARAM_TYPES's declared types (in that order — `from_bridge` is
/// type-directed) and boxes any primitive result, since `Method.invoke`/
/// `Constructor.newInstance` both take a plain `Object[]`.
fn build_invoke_args<'local>(
    env: &mut jni::Env<'local>,
    param_types: &jni::objects::JObjectArray<'local>,
    args: &[crate::protocol::BridgeValue],
) -> jni::errors::Result<jni::objects::JObjectArray<'local>> {
    let object_class = env.find_class(jni::jni_str!("java/lang/Object"))?;
    let invoke_args =
        env.new_object_array(args.len() as i32, &object_class, jni::objects::JObject::null())?;
    for (i, arg) in args.iter().enumerate() {
        let param_type_obj = param_types.get_element(env, i)?;
        let param_type = env.cast_local::<jni::objects::JClass>(param_type_obj)?;
        let jvalue = crate::jvm::convert::from_bridge(env, arg, &param_type)?;
        let boxed = crate::jvm::convert::box_if_primitive(env, jvalue)?;
        invoke_args.set_element(env, i, boxed)?;
    }
    Ok(invoke_args)
}
