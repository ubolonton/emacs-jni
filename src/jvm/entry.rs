//! The `emacs_jni.core` Java shim: defining its classes in the JVM, and the Rust implementations
//! of its `native` methods.
//!
//! A Lisp-access native (`Lisp.*`, `LispObject.prin1ToString`) blocks its JVM thread until the
//! main thread replies. Inside `jvm--call-direct` it runs the request directly instead, on the
//! main thread's [`Env`](emacs::Env) (see [`crate::lisp::runtime`]).

use jni::objects::{JClass, JClassLoader, JObject, JObjectArray, JString, JThrowable, JValue};

// Shim class files, compiled by `build.rs`.
const LISP_CALLABLE_CLASS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/target/emacs-jni-shim-classes/emacs_jni/core/LispCallable.class"
));
const LISP_FUNCTION_CLASS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/target/emacs-jni-shim-classes/emacs_jni/core/LispFunction.class"
));
const LISP_NON_LOCAL_EXIT_CLASS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/target/emacs-jni-shim-classes/emacs_jni/core/LispNonLocalExit.class"
));
const LISP_SIGNAL_CLASS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/target/emacs-jni-shim-classes/emacs_jni/core/LispSignal.class"
));
const LISP_THROW_CLASS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/target/emacs-jni-shim-classes/emacs_jni/core/LispThrow.class"
));
const RELEASE_NOTIFIER_CLASS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/target/emacs-jni-shim-classes/emacs_jni/core/ReleaseNotifier.class"
));
const LISP_OBJECT_CLASS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/target/emacs-jni-shim-classes/emacs_jni/core/LispObject.class"
));
const LISP_CLASS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/target/emacs-jni-shim-classes/emacs_jni/core/Lisp.class"
));
const LISP_REF_RELEASE_NOTIFIER_CLASS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/target/emacs-jni-shim-classes/emacs_jni/core/LispRefReleaseNotifier.class"
));
const LISP_SYMBOL_CLASS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/target/emacs-jni-shim-classes/emacs_jni/core/LispSymbol.class"
));

/// Defines the `emacs_jni.core` shim classes in the attached JVM. Call once, right after the JVM
/// is created, before anything looks up these classes.
///
/// The classes go into the platform classloader. It is the parent of both the system loader (used
/// by `FindClass` with no Java frames on the stack) and the shared jar loader
/// (`jvm::inbox::create_shared_classloader`), so both can see the classes. A class in the
/// system loader is not visible to jar classes, which then fail with `NoClassDefFoundError`.
///
/// Idempotent. jni 0.22.4 keeps one process-global `JavaVM`, so a restart (or a second test)
/// reuses a JVM that already has these classes. Defining a class twice is a `LinkageError`.
pub fn define_classes(env: &mut jni::Env) -> jni::errors::Result<()> {
    // `define_if_absent` checks, then defines. Two concurrent callers on the shared JVM could
    // both define the same class, so serialize the whole function.
    static DEFINE_CLASSES_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = DEFINE_CLASSES_LOCK.lock().unwrap();

    let class_loader_class = env.find_class(jni::jni_str!("java/lang/ClassLoader"))?;
    let platform_loader_obj = env
        .call_static_method(
            &class_loader_class,
            jni::jni_str!("getPlatformClassLoader"),
            jni::jni_sig!(() -> java.lang.ClassLoader),
            &[],
        )?
        .l()?;
    let platform_loader = env.cast_local::<JClassLoader>(platform_loader_obj)?;

    // LispFunction implements LispCallable, so LispCallable must be defined first.
    define_if_absent(env, &platform_loader, "emacs_jni/core/LispCallable", LISP_CALLABLE_CLASS)?;
    define_if_absent(env, &platform_loader, "emacs_jni/core/LispFunction", LISP_FUNCTION_CLASS)?;
    define_if_absent(
        env,
        &platform_loader,
        "emacs_jni/core/LispNonLocalExit",
        LISP_NON_LOCAL_EXIT_CLASS,
    )?;
    // LispSignal and LispThrow extend LispNonLocalExit.
    define_if_absent(env, &platform_loader, "emacs_jni/core/LispSignal", LISP_SIGNAL_CLASS)?;
    define_if_absent(env, &platform_loader, "emacs_jni/core/LispThrow", LISP_THROW_CLASS)?;
    define_if_absent(
        env,
        &platform_loader,
        "emacs_jni/core/ReleaseNotifier",
        RELEASE_NOTIFIER_CLASS,
    )?;
    define_if_absent(
        env,
        &platform_loader,
        "emacs_jni/core/LispRefReleaseNotifier",
        LISP_REF_RELEASE_NOTIFIER_CLASS,
    )?;
    define_if_absent(env, &platform_loader, "emacs_jni/core/LispObject", LISP_OBJECT_CLASS)?;
    define_if_absent(env, &platform_loader, "emacs_jni/core/Lisp", LISP_CLASS)?;
    define_if_absent(env, &platform_loader, "emacs_jni/core/LispSymbol", LISP_SYMBOL_CLASS)?;

    // Confirm that `find_class` sees the new classes.
    env.find_class(jni::jni_str!("emacs_jni/core/LispCallable"))?;
    env.find_class(jni::jni_str!("emacs_jni/core/LispFunction"))?;
    env.find_class(jni::jni_str!("emacs_jni/core/LispNonLocalExit"))?;
    env.find_class(jni::jni_str!("emacs_jni/core/LispSignal"))?;
    env.find_class(jni::jni_str!("emacs_jni/core/LispThrow"))?;
    env.find_class(jni::jni_str!("emacs_jni/core/ReleaseNotifier"))?;
    env.find_class(jni::jni_str!("emacs_jni/core/LispRefReleaseNotifier"))?;
    env.find_class(jni::jni_str!("emacs_jni/core/LispObject"))?;
    env.find_class(jni::jni_str!("emacs_jni/core/Lisp"))?;
    env.find_class(jni::jni_str!("emacs_jni/core/LispSymbol"))?;

    register_lisp_function_native(env)?;
    register_release_notifier_native(env)?;
    register_lisp_native(env)?;
    register_lisp_object_native(env)?;
    register_lisp_ref_release_notifier_native(env)?;

    Ok(())
}

/// Registers the native implementation of `LispFunction.call`.
///
/// `native_method!` generates the `extern "system"` wrapper and checks the signature at compile
/// time. Its error policy throws a `RuntimeException` for a returned `jni::errors::Error`. That
/// happens only if the request cannot be sent; a failed Lisp callback completes the future
/// exceptionally instead.
// FIX: Rust 1.99 deprecates `AtomicBool::fetch_update`, which `native_method!` expands to in
// jni 0.22.4. Remove the `allow`s when jni stops using it. `expect` would fail on Rust 1.98.
#[allow(deprecated)]
fn register_lisp_function_native(env: &mut jni::Env) -> jni::errors::Result<()> {
    let class = env.find_class(jni::jni_str!("emacs_jni/core/LispFunction"))?;
    const CALL_METHOD: jni::NativeMethod = jni::native_method! {
        name = "call",
        fn lisp_function_call(args: [java.lang.Object]) -> java.util.concurrent.CompletableFuture,
    };
    // SAFETY: `native_method!` checks that `lisp_function_call` matches the JNI signature, and
    // `call` is an instance method on `LispFunction`, as `register_native_methods` requires.
    unsafe { env.register_native_methods(&class, &[CALL_METHOD]) }
}

/// Implements `LispFunction.call`: registers a new `CompletableFuture`, enqueues an
/// [`Inbound::Invoke`](crate::lisp::inbox::Inbound::Invoke), and returns the future. Does not
/// block. Emacs runs the Lisp function later on its main thread, and the result arrives as
/// [`Cmd::InvokeReply`](crate::jvm::inbox::Cmd::InvokeReply).
fn lisp_function_call<'local>(
    env: &mut jni::Env<'local>,
    this: JObject<'local>,
    args: JObjectArray<'local>,
) -> jni::errors::Result<JObject<'local>> {
    let handle = get_handle_field(env, &this)?;
    let args = crate::jvm::convert::jobject_array_to_bridge_values(env, &args)?;

    // Inside `jvm--call-direct`, the Java caller may wait on the result on this thread, which is
    // the one that drains the queue. Run the callback directly and return a completed future.
    if crate::lisp::runtime::in_scope() {
        let outcome = crate::lisp::runtime::with_current_env(|lisp_env| {
            crate::lisp::inbox::invoke_callback(lisp_env, handle, &args)
        })
        .ok_or_else(|| {
            crate::jvm::convert::marshal_error("reentrant callback lost its Env scope")
        })?;
        let future_class =
            env.find_class(jni::jni_str!("java/util/concurrent/CompletableFuture"))?;
        let future = env.new_object(&future_class, jni::jni_sig!(() -> void), &[])?;
        crate::jvm::invoke::complete_future_with_reply(env, &future, outcome)?;
        return Ok(future);
    }

    let id = crate::jvm::inbox::next_id();

    let future_class = env.find_class(jni::jni_str!("java/util/concurrent/CompletableFuture"))?;
    let future = env.new_object(&future_class, jni::jni_sig!(() -> void), &[])?;
    let future_global = env.new_global_ref(&future)?;
    crate::jvm::inbox::register(id, future_global);

    crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::Invoke { id, handle, args });

    Ok(future)
}

/// Registers the native implementation of `ReleaseNotifier.run`.
// FIX: Deprecated `native_method!` expansion; see `register_lisp_function_native`.
#[allow(deprecated)]
fn register_release_notifier_native(env: &mut jni::Env) -> jni::errors::Result<()> {
    let class = env.find_class(jni::jni_str!("emacs_jni/core/ReleaseNotifier"))?;
    const RUN_METHOD: jni::NativeMethod = jni::native_method! {
        name = "run",
        fn release_notifier_run() -> void,
    };
    // SAFETY: as in `register_lisp_function_native`; `run` is an instance method on
    // `ReleaseNotifier`.
    unsafe { env.register_native_methods(&class, &[RUN_METHOD]) }
}

/// Implements `ReleaseNotifier.run`. The `LispFunction` this notifier watches was collected, so
/// tell Lisp to drop its handle from the function table. Runs on a `Cleaner` thread.
fn release_notifier_run<'local>(
    env: &mut jni::Env<'local>,
    this: JObject<'local>,
) -> jni::errors::Result<()> {
    let handle = get_handle_field(env, &this)?;
    crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::Release { handle });
    Ok(())
}

/// Reads the private `long handle` field of a `LispFunction` or `ReleaseNotifier`. JNI field
/// access ignores Java access control.
fn get_handle_field(env: &mut jni::Env<'_>, this: &JObject<'_>) -> jni::errors::Result<u64> {
    let value = env.get_field(this, jni::jni_str!("handle"), jni::jni_sig!(long))?.j()?;
    Ok(value as u64)
}

/// Defines the class `name` (internal form, e.g. `emacs_jni/core/Lisp`) from `bytecode` in
/// `loader`, unless `FindClass` already finds it.
fn define_if_absent(
    env: &mut jni::Env,
    loader: &JClassLoader<'_>,
    name: &str,
    bytecode: &[u8],
) -> jni::errors::Result<()> {
    let jni_name = jni::strings::JNIString::from(name);
    // `find_class` returns `Err` for a missing class and leaves no pending exception.
    if env.find_class(&jni_name).is_ok() {
        return Ok(());
    }
    env.define_class(Some(&jni_name), loader, bytecode)?;
    Ok(())
}

/// How long a synchronous Lisp-access native waits for its reply. Emacs may be busy.
const LISP_REQUEST_TIMEOUT_SECS: i64 = 30;

/// Registers the static natives on `emacs_jni.core.Lisp`, which Clojure uses to access Lisp
/// values.
// FIX: Deprecated `native_method!` expansion; see `register_lisp_function_native`.
#[allow(deprecated)]
fn register_lisp_native(env: &mut jni::Env) -> jni::errors::Result<()> {
    let class = env.find_class(jni::jni_str!("emacs_jni/core/Lisp"))?;
    // Parameters are `Object`: a value can be a `LispObject`, a `LispSymbol`, or a boxed scalar.
    // `jvm::convert::to_input` classifies it.
    const INTERN_METHOD: jni::NativeMethod = jni::native_method! {
        name = "intern",
        static fn lisp_intern(name: java.lang.String) -> java.lang.Object,
    };
    const TYPE_OF_METHOD: jni::NativeMethod = jni::native_method! {
        name = "typeOf",
        static fn lisp_type_of(v: java.lang.Object) -> java.lang.Object,
    };
    const IS_NOT_NIL_METHOD: jni::NativeMethod = jni::native_method! {
        name = "isNotNil",
        static fn lisp_is_not_nil(v: java.lang.Object) -> boolean,
    };
    const EQ_METHOD: jni::NativeMethod = jni::native_method! {
        name = "eq",
        static fn lisp_eq(a: java.lang.Object, b: java.lang.Object) -> boolean,
    };
    const APPLY_ASYNC_METHOD: jni::NativeMethod = jni::native_method! {
        name = "applyAsync",
        static fn lisp_apply_async(f: java.lang.Object, args: [java.lang.Object]) -> java.util.concurrent.CompletableFuture,
    };
    // SAFETY: `native_method!` checks each function against its JNI signature, and each name is a
    // static method on `Lisp`, as `register_native_methods` requires.
    unsafe {
        env.register_native_methods(
            &class,
            &[INTERN_METHOD, TYPE_OF_METHOD, IS_NOT_NIL_METHOD, EQ_METHOD, APPLY_ASYNC_METHOD],
        )
    }
}

/// Registers `LispObject.prin1ToString`'s native implementation.
// FIX: Deprecated `native_method!` expansion; see `register_lisp_function_native`.
#[allow(deprecated)]
fn register_lisp_object_native(env: &mut jni::Env) -> jni::errors::Result<()> {
    let class = env.find_class(jni::jni_str!("emacs_jni/core/LispObject"))?;
    const PRIN1_TO_STRING_METHOD: jni::NativeMethod = jni::native_method! {
        name = "prin1ToString",
        fn lisp_object_prin1_to_string() -> java.lang.String,
    };
    // SAFETY: as in `register_lisp_function_native`; `prin1ToString` is an instance method on
    // `LispObject`.
    unsafe { env.register_native_methods(&class, &[PRIN1_TO_STRING_METHOD]) }
}

/// Registers `LispRefReleaseNotifier.run`'s native implementation.
// FIX: Deprecated `native_method!` expansion; see `register_lisp_function_native`.
#[allow(deprecated)]
fn register_lisp_ref_release_notifier_native(env: &mut jni::Env) -> jni::errors::Result<()> {
    let class = env.find_class(jni::jni_str!("emacs_jni/core/LispRefReleaseNotifier"))?;
    const RUN_METHOD: jni::NativeMethod = jni::native_method! {
        name = "run",
        fn lisp_ref_release_notifier_run() -> void,
    };
    // SAFETY: as in `register_lisp_function_native`; `run` is an instance method on
    // `LispRefReleaseNotifier`.
    unsafe { env.register_native_methods(&class, &[RUN_METHOD]) }
}

/// Sends the Lisp-access request that `build_request` makes from an id, and blocks this JVM
/// thread on the reply for up to [`LISP_REQUEST_TIMEOUT_SECS`]. Inside `jvm--call-direct`, runs
/// it directly instead ([`service_lisp_request_direct`]), with id 0.
fn service_lisp_request_sync<'local>(
    env: &mut jni::Env<'local>,
    build_request: impl FnOnce(u64) -> crate::lisp::inbox::Inbound,
) -> jni::errors::Result<JObject<'local>> {
    // Inside `jvm--call-direct` on the main thread, the queue cannot drain: this thread is the
    // one that drains it. Run the request directly.
    if crate::lisp::runtime::in_scope() {
        return service_lisp_request_direct(env, build_request(0));
    }
    let id = crate::jvm::inbox::next_id();
    let future_class = env.find_class(jni::jni_str!("java/util/concurrent/CompletableFuture"))?;
    let future = env.new_object(&future_class, jni::jni_sig!(() -> void), &[])?;
    let future_global = env.new_global_ref(&future)?;
    crate::jvm::inbox::register(id, future_global);

    crate::lisp::inbox::submit(build_request(id));

    block_on_lisp_reply(env, &future)
}

/// Runs a Lisp-access `request` on the [`Env`](emacs::Env) of the enclosing `jvm--call-direct`.
/// Returns the Java result, or throws the Lisp signal/throw as a `LispNonLocalExit`.
fn service_lisp_request_direct<'local>(
    env: &mut jni::Env<'local>,
    request: crate::lisp::inbox::Inbound,
) -> jni::errors::Result<JObject<'local>> {
    let reply = crate::lisp::runtime::with_current_env(|lisp_env| {
        crate::lisp::inbox::compute_reply(lisp_env, &request)
    })
    // `in_scope()` was true on this thread, so the scope is still pushed.
    .ok_or_else(|| crate::jvm::convert::marshal_error("reentrant Lisp call lost its Env scope"))?;

    match reply {
        crate::protocol::LispOutcome::Signal { symbol, data } => {
            Err(crate::jvm::invoke::throw_non_local_exit(
                env,
                crate::protocol::NON_LOCAL_EXIT_SIGNAL,
                *symbol,
                *data,
            ))
        }
        crate::protocol::LispOutcome::Throw { tag, value } => {
            Err(crate::jvm::invoke::throw_non_local_exit(
                env,
                crate::protocol::NON_LOCAL_EXIT_THROW,
                *tag,
                *value,
            ))
        }
        other => crate::jvm::invoke::build_lisp_reply_object(env, other),
    }
}

/// Blocks on `future.get(LISP_REQUEST_TIMEOUT_SECS, SECONDS)`. See
/// [`LISP_REQUEST_TIMEOUT_SECS`].
fn block_on_lisp_reply<'local>(
    env: &mut jni::Env<'local>,
    future: &JObject<'local>,
) -> jni::errors::Result<JObject<'local>> {
    let time_unit_class = env.find_class(jni::jni_str!("java/util/concurrent/TimeUnit"))?;
    let seconds = env
        .get_static_field(
            &time_unit_class,
            jni::jni_str!("SECONDS"),
            jni::jni_sig!(java.util.concurrent.TimeUnit),
        )?
        .l()?;
    match env.call_method(
        future,
        jni::jni_str!("get"),
        jni::jni_sig!((timeout: long, unit: java.util.concurrent.TimeUnit) -> java.lang.Object),
        &[JValue::Long(LISP_REQUEST_TIMEOUT_SECS), JValue::Object(&seconds)],
    ) {
        Ok(v) => v.l(),
        Err(jni::errors::Error::JavaException) => Err(rethrow_lisp_reply_exception(env)),
        Err(e) => Err(e),
    }
}

/// Rethrows the pending exception from a failed `CompletableFuture#get`. An `ExecutionException`
/// is unwrapped to its cause (the `LispNonLocalExit`). `TimeoutException` and
/// `InterruptedException` are rethrown as they are.
///
/// Returns `Err(Error::JavaException)` with the exception pending, so `native_method!` does not
/// wrap it in a `RuntimeException`.
fn rethrow_lisp_reply_exception(env: &mut jni::Env) -> jni::errors::Error {
    let caught = match env.exception_catch() {
        Err(e) => e,
        Ok(()) => {
            return crate::jvm::convert::marshal_error(
                "expected a pending exception from a Lisp-reply future's get() but found none",
            )
        }
    };
    let jni::errors::Error::CaughtJavaException { exception, .. } = caught else {
        return caught;
    };
    let throwable = match env
        .new_local_ref(exception.as_obj())
        .and_then(|local| env.cast_local::<JThrowable>(local))
    {
        Ok(t) => t,
        Err(e) => return e,
    };
    let to_rethrow = match throwable.get_cause(env) {
        Ok(cause) if !cause.is_null() => cause,
        _ => throwable,
    };
    match env.throw(&to_rethrow) {
        Err(e) => e,
        Ok(()) => {
            crate::jvm::convert::marshal_error("env.throw unexpectedly left no pending exception")
        }
    }
}

/// `Lisp.intern`'s native implementation.
fn lisp_intern<'local>(
    env: &mut jni::Env<'local>,
    _class: JClass<'local>,
    name: JString<'local>,
) -> jni::errors::Result<JObject<'local>> {
    let name = name.try_to_string(env)?;
    service_lisp_request_sync(env, |id| crate::lisp::inbox::Inbound::Intern { id, name })
}

/// `Lisp.typeOf`'s native implementation.
fn lisp_type_of<'local>(
    env: &mut jni::Env<'local>,
    _class: JClass<'local>,
    v: JObject<'local>,
) -> jni::errors::Result<JObject<'local>> {
    let arg = crate::jvm::convert::to_input(env, v)?;
    service_lisp_request_sync(env, |id| crate::lisp::inbox::Inbound::TypeOf { id, arg })
}

/// `Lisp.isNotNil`'s native implementation.
fn lisp_is_not_nil<'local>(
    env: &mut jni::Env<'local>,
    _class: JClass<'local>,
    v: JObject<'local>,
) -> jni::errors::Result<bool> {
    let arg = crate::jvm::convert::to_input(env, v)?;
    let result =
        service_lisp_request_sync(env, |id| crate::lisp::inbox::Inbound::IsNotNil { id, arg })?;
    unbox_boolean(env, &result)
}

/// `Lisp.eq`'s native implementation.
fn lisp_eq<'local>(
    env: &mut jni::Env<'local>,
    _class: JClass<'local>,
    a: JObject<'local>,
    b: JObject<'local>,
) -> jni::errors::Result<bool> {
    let a = crate::jvm::convert::to_input(env, a)?;
    let b = crate::jvm::convert::to_input(env, b)?;
    let result = service_lisp_request_sync(env, |id| crate::lisp::inbox::Inbound::Eq { id, a, b })?;
    unbox_boolean(env, &result)
}

/// `Lisp.applyAsync`'s native implementation. Returns the future without blocking.
fn lisp_apply_async<'local>(
    env: &mut jni::Env<'local>,
    _class: JClass<'local>,
    f: JObject<'local>,
    args: JObjectArray<'local>,
) -> jni::errors::Result<JObject<'local>> {
    let f = crate::jvm::convert::to_input(env, f)?;
    let request_args = crate::jvm::convert::jobject_array_to_lisp_request_args(env, &args)?;

    // Inside `jvm--call-direct`, run the apply directly and return a completed future.
    if crate::lisp::runtime::in_scope() {
        let request = crate::lisp::inbox::Inbound::Apply { id: 0, f, args: request_args };
        let reply = crate::lisp::runtime::with_current_env(|lisp_env| {
            crate::lisp::inbox::compute_reply(lisp_env, &request)
        })
        .ok_or_else(|| crate::jvm::convert::marshal_error("reentrant apply lost its Env scope"))?;
        let future_class =
            env.find_class(jni::jni_str!("java/util/concurrent/CompletableFuture"))?;
        let future = env.new_object(&future_class, jni::jni_sig!(() -> void), &[])?;
        crate::jvm::invoke::complete_future_with_reply(env, &future, reply)?;
        return Ok(future);
    }

    let id = crate::jvm::inbox::next_id();
    let future_class = env.find_class(jni::jni_str!("java/util/concurrent/CompletableFuture"))?;
    let future = env.new_object(&future_class, jni::jni_sig!(() -> void), &[])?;
    let future_global = env.new_global_ref(&future)?;
    crate::jvm::inbox::register(id, future_global);

    crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::Apply { id, f, args: request_args });
    Ok(future)
}

/// `LispObject.prin1ToString`'s native implementation.
fn lisp_object_prin1_to_string<'local>(
    env: &mut jni::Env<'local>,
    this: JObject<'local>,
) -> jni::errors::Result<JString<'local>> {
    let ptr = get_ptr_field(env, &this)?;
    let result =
        service_lisp_request_sync(env, |id| crate::lisp::inbox::Inbound::Prin1 { id, ptr })?;
    env.cast_local::<JString>(result)
}

/// `LispRefReleaseNotifier.run`'s native implementation: enqueues an
/// [`Inbound::FreeRef`](crate::lisp::inbox::Inbound::FreeRef) for the `ptr` of `this`.
fn lisp_ref_release_notifier_run<'local>(
    env: &mut jni::Env<'local>,
    this: JObject<'local>,
) -> jni::errors::Result<()> {
    let ptr = get_ptr_field(env, &this)?;
    crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::FreeRef { ptr });
    Ok(())
}

/// Unboxes a `java.lang.Boolean` to a `bool`.
fn unbox_boolean(env: &mut jni::Env, obj: &JObject) -> jni::errors::Result<bool> {
    env.call_method(obj, jni::jni_str!("booleanValue"), jni::jni_sig!(() -> boolean), &[])?.z()
}

/// Reads the private `ptr` field of `this`, a `LispObject` or `LispRefReleaseNotifier`. JNI field
/// access ignores Java access control.
pub(crate) fn get_ptr_field(
    env: &mut jni::Env<'_>,
    this: &JObject<'_>,
) -> jni::errors::Result<i64> {
    env.get_field(this, jni::jni_str!("ptr"), jni::jni_sig!(long))?.j()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jvm::test_support::create_test_jvm;

    #[test]
    fn defines_shim_classes_and_they_are_findable() {
        let vm = create_test_jvm();
        vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            define_classes(env)?;
            // LispFunction must implement LispCallable.
            let lisp_function_class =
                env.find_class(jni::jni_str!("emacs_jni/core/LispFunction"))?;
            let lisp_callable_class =
                env.find_class(jni::jni_str!("emacs_jni/core/LispCallable"))?;
            let is_assignable =
                env.is_assignable_from(&lisp_function_class, &lisp_callable_class)?;
            assert!(is_assignable, "LispFunction should implement LispCallable");
            // Both release notifiers must implement Runnable.
            let runnable_class = env.find_class(jni::jni_str!("java/lang/Runnable"))?;
            let release_notifier_class =
                env.find_class(jni::jni_str!("emacs_jni/core/ReleaseNotifier"))?;
            assert!(
                env.is_assignable_from(&release_notifier_class, &runnable_class)?,
                "ReleaseNotifier should implement Runnable"
            );
            let lisp_ref_release_notifier_class =
                env.find_class(jni::jni_str!("emacs_jni/core/LispRefReleaseNotifier"))?;
            assert!(
                env.is_assignable_from(&lisp_ref_release_notifier_class, &runnable_class)?,
                "LispRefReleaseNotifier should implement Runnable"
            );
            Ok(())
        })
        .expect("defining shim classes into a fresh JVM should succeed");
    }

    #[test]
    fn get_ptr_field_reads_back_what_was_stored() {
        let vm = create_test_jvm();
        vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            define_classes(env)?;
            let lisp_object_class = env.find_class(jni::jni_str!("emacs_jni/core/LispObject"))?;
            // JNI ignores the package-private access of this constructor.
            let obj = env.new_object(
                &lisp_object_class,
                jni::jni_sig!((ptr: long) -> void),
                &[JValue::Long(1234)],
            )?;
            assert_eq!(get_ptr_field(env, &obj)?, 1234);
            Ok(())
        })
        .expect("constructing a LispObject and reading its ptr field back should succeed");
    }
}
