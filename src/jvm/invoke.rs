//! Reflective method and constructor calls, shared by the async worker path and the synchronous
//! `jvm--call-direct` path. Also converts Lisp replies to Java values or thrown
//! `LispSignal`/`LispThrow`.

use std::sync::Arc;

use jni::vm::JavaVM;

use crate::jvm::convert::marshal_error;
use crate::protocol::{NON_LOCAL_EXIT_SIGNAL, NON_LOCAL_EXIT_THROW};

/// Builds the Java value for a successful [`LispOutcome`](crate::protocol::LispOutcome). Panics on
/// `Signal`/`Throw`: callers handle them first.
pub(crate) fn build_lisp_reply_object<'local>(
    env: &mut jni::Env<'local>,
    value: crate::protocol::LispOutcome,
) -> jni::errors::Result<jni::objects::JObject<'local>> {
    match value {
        crate::protocol::LispOutcome::Value(bridge) => {
            // Same conversion as a callback return value in `handle_invoke_reply`.
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

/// Completes `future` with `value`: exceptionally with a `LispNonLocalExit` for `Signal`/`Throw`,
/// normally otherwise. Used by both the async and the direct path, so Clojure's `apply` sees the
/// same result.
pub(crate) fn complete_future_with_reply<'local>(
    env: &mut jni::Env<'local>,
    future: &jni::objects::JObject<'local>,
    value: crate::protocol::LispOutcome,
) -> jni::errors::Result<()> {
    match value {
        crate::protocol::LispOutcome::Signal { symbol, data } => {
            let ex = build_non_local_exit(env, NON_LOCAL_EXIT_SIGNAL, *symbol, *data)?;
            env.call_method(
                future,
                jni::jni_str!("completeExceptionally"),
                jni::jni_sig!((ex: java.lang.Throwable) -> boolean),
                &[jni::objects::JValueOwned::Object(ex).borrow()],
            )?;
        }
        crate::protocol::LispOutcome::Throw { tag, value } => {
            let ex = build_non_local_exit(env, NON_LOCAL_EXIT_THROW, *tag, *value)?;
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

/// Builds a `LispSignal` ([`NON_LOCAL_EXIT_SIGNAL`]) or `LispThrow` ([`NON_LOCAL_EXIT_THROW`])
/// from two classified values: symbol/data or tag/value.
fn build_non_local_exit<'local>(
    env: &mut jni::Env<'local>,
    kind: i32,
    a: crate::protocol::LispOutcome,
    b: crate::protocol::LispOutcome,
) -> jni::errors::Result<jni::objects::JObject<'local>> {
    let a_obj = build_lisp_reply_object(env, a)?;
    let b_obj = build_lisp_reply_object(env, b)?;
    let class = if kind == NON_LOCAL_EXIT_SIGNAL {
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

/// Throws a `LispNonLocalExit` from a direct-path native, so it propagates up the Clojure stack
/// as on the async path. Returns [`Error::JavaException`](jni::errors::Error::JavaException), so
/// that `native_method!` keeps this pending exception instead of throwing a `RuntimeException`
/// over it.
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

/// Handles a [`Cmd::Call`](crate::jvm::inbox::Cmd::Call): invokes `target.member(args)`, then
/// enqueues [`Inbound::CallDone`](crate::lisp::inbox::Inbound::CallDone). Runs on its own thread,
/// so a slow Java call does not block the worker.
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
            crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::CallDone {
                id,
                ok: true,
                result: value,
                err: None,
            });
        }
        Err(e) => {
            crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::CallDone {
                id,
                ok: false,
                result: crate::protocol::BridgeValue::Nil,
                err: Some(e.to_string()),
            });
        }
    }
}

/// Resolves `target.member` against the shared classloader and invokes it with `args`. The thread
/// must be attached. Used by both [`handle_call`] and [`call_direct`].
pub(crate) fn resolve_and_invoke(
    env: &mut jni::Env,
    loader: &Option<crate::jvm::runtime::SharedLoader>,
    target: &crate::protocol::CallTarget,
    member: &str,
    args: &[crate::protocol::BridgeValue],
) -> jni::errors::Result<crate::protocol::BridgeValue> {
    // Clojure's `RT` loads `.clj` files as resources through the context classloader, which is
    // null on a newly attached thread. Without this, the first use of Clojure throws
    // `ExceptionInInitializerError`.
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
                // Jar classes are visible only through the shared loader, not `FindClass`. The
                // loader delegates to its parent first, so JDK classes still resolve.
                Some(global) => {
                    let loader_local = env.new_local_ref(global.as_obj())?;
                    resolve_class_via_loader(env, &loader_local, name)?
                }
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
    /// Nesting depth of [`call_direct`] frames on this thread. While it is `> 0`,
    /// [`describe_caught_java_exception`] stores a `LispNonLocalExit` in [`PENDING_NLE`].
    static ATTACH_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };

    /// A `LispNonLocalExit` that reached the top of a [`call_direct`], so that [`call_direct`] can
    /// re-raise the original signal/throw instead of `jvm-error`. Set by
    /// [`describe_caught_java_exception`], which can only return a [`jni::errors::Error`].
    static PENDING_NLE: std::cell::RefCell<Option<DirectError>> =
        const { std::cell::RefCell::new(None) };
}

/// Failure of a [`call_direct`].
pub(crate) enum DirectError {
    /// A Lisp signal/throw that unwound through Clojure. `kind` is [`NON_LOCAL_EXIT_SIGNAL`] or
    /// [`NON_LOCAL_EXIT_THROW`]; `a`/`b` are symbol/data or tag/value.
    NonLocalExit { kind: i32, a: crate::protocol::LispInput, b: crate::protocol::LispInput },
    /// Any other failure. Signalled as `jvm-error`.
    Jni(jni::errors::Error),
}

/// Runs `target.member(args)` synchronously on the current thread. The Emacs main thread attaches
/// to the JVM permanently; other threads attach for this call only.
pub(crate) fn call_direct(
    vm: &JavaVM,
    loader: &Option<crate::jvm::runtime::SharedLoader>,
    target: &crate::protocol::CallTarget,
    member: &str,
    args: &[crate::protocol::BridgeValue],
) -> Result<crate::protocol::BridgeValue, DirectError> {
    ATTACH_DEPTH.with(|d| d.set(d.get() + 1));
    // Clear a stale value, so a value after the invoke belongs to this frame.
    PENDING_NLE.with(|s| *s.borrow_mut() = None);

    // A permanent attach keeps the main thread's `java.lang.Thread` stable across calls, so
    // `Thread.currentThread()` and thread-locals (Clojure dynamic bindings, `*ns*`) persist.
    // Other threads (e.g. `make-thread`) are not attached here, so the scoped attach below
    // detaches them on return, and `destroy` is not blocked.
    if crate::jvm::runtime::on_main_thread()
        && !crate::jvm::runtime::main_attached()
        && vm.attach_current_thread(|_env| Ok::<(), jni::errors::Error>(())).is_ok()
    {
        crate::jvm::runtime::set_main_attached(true);
    }

    let result = vm.attach_current_thread_for_scope(|env| {
        resolve_and_invoke(env, loader, target, member, args)
    });

    ATTACH_DEPTH.with(|d| d.set(d.get() - 1));
    match result {
        Ok(arg) => Ok(arg),
        // Re-raise a Lisp signal/throw as itself. Anything else is `jvm-error`.
        Err(e) => Err(PENDING_NLE.with(|s| s.borrow_mut().take()).unwrap_or(DirectError::Jni(e))),
    }
}

/// Resolves `name`, a binary class name (e.g. `"java.lang.String"`), with `loader.loadClass`.
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

/// Describes a caught Java exception, including its whole `getCause()` chain.
///
/// `Method.invoke` and `Constructor.newInstance` wrap the target's exception in an
/// `InvocationTargetException` with a null message. The default description shows only that
/// wrapper, so the real cause is lost.
///
/// The caller catches the exception, because it must first check [`is_overload_arg_mismatch`].
///
/// Inside [`call_direct`], also stores a `LispNonLocalExit` in the cause chain in
/// [`PENDING_NLE`].
fn describe_caught_java_exception(
    env: &mut jni::Env,
    caught: jni::errors::Error,
) -> jni::errors::Error {
    let jni::errors::Error::CaughtJavaException { name, msg, exception, .. } = caught else {
        return caught;
    };

    // Inside a direct call, keep a `LispNonLocalExit` so `call_direct` can re-raise it. On
    // failure, fall through to the message.
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
            // Bounded: a hostile `Throwable` subclass could make a cause cycle.
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

/// True if `caught` is a bare `IllegalArgumentException` from `Method.invoke` or
/// `Constructor.newInstance`: the arguments do not fit the candidate's parameter types. An
/// exception from the invoked code arrives as `InvocationTargetException`, so this does not match
/// it.
fn is_overload_arg_mismatch(caught: &jni::errors::Error) -> bool {
    matches!(
        caught,
        jni::errors::Error::CaughtJavaException { name, .. }
            if name == "java.lang.IllegalArgumentException"
    )
}

/// Finds a `LispNonLocalExit` in the cause chain of `throwable` and converts it to
/// [`DirectError::NonLocalExit`]. Returns `None` for any other exception.
fn extract_non_local_exit<'local>(
    env: &mut jni::Env<'local>,
    throwable: jni::objects::JThrowable<'local>,
) -> jni::errors::Result<Option<DirectError>> {
    let nle_class = env.find_class(jni::jni_str!("emacs_jni/core/LispNonLocalExit"))?;
    let mut current = throwable;
    // Bounded, as in `describe_caught_java_exception`.
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

/// Invokes the first public `method_name` on `class` that accepts `args`, and marshals the result.
/// `instance` is the receiver, or `None` for a static method. Candidates with the correct arity
/// are tried in `getMethods()` order. A candidate is skipped when `args` do not marshal to its
/// parameter types, or when `invoke` rejects them. There is no best-fit scoring; see
/// `docs/explanation/limitations-and-tradeoffs.md`.
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

    // The last argument mismatch, for the "no method found" error.
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

        // `args` do not fit this candidate. Try the next one.
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
            // `args` do not fit this candidate. Try the next one. Any other exception is a real
            // failure.
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

/// Invokes the first public constructor of `class` that accepts `args`. Overloads are tried as
/// in [`invoke_method`].
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

    // The last argument mismatch, for the "no constructor found" error.
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

        // `args` do not fit this candidate. Try the next one.
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
            // `args` do not fit this candidate. Try the next one. Any other exception is a real
            // failure.
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

/// Marshals `args` to `param_types` and boxes primitives, for `Method.invoke` and
/// `Constructor.newInstance`.
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
