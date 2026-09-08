use jni::objects::{JClass, JClassLoader, JObject, JObjectArray, JString, JThrowable, JValue, JValueOwned};

const LISP_CALLABLE_CLASS: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/target/emacs-jni-shim-classes/emacs_jni/LispCallable.class"));
const LISP_FUNCTION_CLASS: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/target/emacs-jni-shim-classes/emacs_jni/LispFunction.class"));
const LISP_ERROR_CLASS: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/target/emacs-jni-shim-classes/emacs_jni/LispError.class"));
const RELEASE_NOTIFIER_CLASS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/target/emacs-jni-shim-classes/emacs_jni/ReleaseNotifier.class"
));
const LISP_OBJECT_CLASS: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/target/emacs-jni-shim-classes/emacs_jni/LispObject.class"));
const LISP_CLASS: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/target/emacs-jni-shim-classes/emacs_jni/Lisp.class"));
const LISP_REF_RELEASE_NOTIFIER_CLASS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/target/emacs-jni-shim-classes/emacs_jni/LispRefReleaseNotifier.class"
));

/// Defines the `emacs_jni.{LispCallable,LispFunction,LispError,
/// ReleaseNotifier,LispObject,Lisp,LispRefReleaseNotifier}` shim classes
/// into the currently-attached JVM, using the *platform* classloader
/// (not the system/application classloader, despite this file's original
/// Task 1 comment saying otherwise — see below) so they're visible both
/// to `find_class` calls made with no Java frames on the stack (which JNI
/// routes through the system classloader by default -- see `FindClass`'s
/// documented fallback behavior) and to classes loaded through
/// `jvm_thread::create_shared_classloader`'s `URLClassLoader` (Task 3),
/// whose parent is deliberately the platform loader, not the system
/// loader (see that function's own doc comment). Standard `ClassLoader`
/// delegation is parent-first (a loader asks its parent before itself),
/// and the system loader's own parent is the platform loader, so a class
/// defined only in the platform loader is reachable from *both* paths via
/// ordinary delegation — a class defined in the system loader, as this
/// function originally did, is not reachable from the shared
/// `URLClassLoader` side (loader parents aren't searched by their
/// children's siblings), which was a real bug this task found: any jar
/// class using a shim type (e.g. a method parameter typed
/// `emacs_jni.LispCallable`) threw `NoClassDefFoundError` wrapped in
/// `InvocationTargetException` at the first `Method.invoke`, since the
/// jar's own `URLClassLoader` could never resolve the shim classes that
/// existed only over in the system loader. Called from
/// `jvm_thread::create_jvm`, right after the JVM is created, before
/// anything else in this crate references those classes.
///
/// Idempotent: skips a class that's already defined rather than calling
/// `Env::define_class` on it again, which would throw (duplicate class
/// definition in the same classloader is a `LinkageError`). This matters
/// because `create_jvm` can run more than once against the very same
/// underlying JVM within one process — jni-rs 0.22.4 caches the raw
/// `JavaVM` pointer in a process-global singleton (see `create_jvm`'s own
/// comment on this), so restarting after `jvm-stop`/a crash, or (in this
/// crate's own test suite) any second `cargo test` case that creates its
/// own `JavaVM`, transparently reattaches to a JVM that may already have
/// these classes defined from a previous `create_jvm` call.
pub fn define_classes(env: &mut jni::Env) -> jni::errors::Result<()> {
    // Serializes the whole function: `define_if_absent`'s "find_class,
    // then maybe define_class" is a check-then-act pair, not atomic on its
    // own. The shared JVM singleton (`JavaVM::new` re-attaches to one
    // process-wide `JavaVM` -- see this function's own doc comment) means
    // two threads can both reach `define_classes` at once (e.g. two
    // `cargo test` cases, one via this module's own tests, another via a
    // real `create_jvm` call elsewhere): both see "not yet defined" for
    // the same class and both call `define_class`, and the loser gets a
    // genuine `LinkageError` (duplicate class definition), surfacing here
    // as an opaque JNI error. A single process-wide lock around the
    // entire function turns "two concurrent first calls" into "one first
    // call, then one call that finds everything already defined" --
    // exactly the idempotent case this function already handles.
    static DEFINE_CLASSES_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = DEFINE_CLASSES_LOCK.lock().unwrap();

    // Matches `create_shared_classloader`'s reflective style in
    // jvm_thread.rs (find_class + call_static_method) rather than the
    // jni-rs 0.22.4 typed `JClassLoader::get_platform_class_loader(env)`
    // convenience binding, to keep one JNI-calling style across the crate
    // -- and reuses that same function's exact `getPlatformClassLoader`
    // call, for the reason explained above.
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

    // LispCallable must be defined before LispFunction (which implements
    // it) for the JVM's verifier to resolve the interface reference.
    // LispError and ReleaseNotifier have no such dependency and can go
    // anywhere.
    define_if_absent(env, &platform_loader, "emacs_jni/LispCallable", LISP_CALLABLE_CLASS)?;
    define_if_absent(env, &platform_loader, "emacs_jni/LispFunction", LISP_FUNCTION_CLASS)?;
    define_if_absent(env, &platform_loader, "emacs_jni/LispError", LISP_ERROR_CLASS)?;
    define_if_absent(env, &platform_loader, "emacs_jni/ReleaseNotifier", RELEASE_NOTIFIER_CLASS)?;
    // LispRefReleaseNotifier has no class-loading dependency on LispObject
    // (LispObject's constructor only *references* it inside a method body,
    // which the verifier resolves lazily) -- defined first anyway, out of
    // the same caution as LispCallable/LispFunction's ordering above.
    define_if_absent(env, &platform_loader, "emacs_jni/LispRefReleaseNotifier", LISP_REF_RELEASE_NOTIFIER_CLASS)?;
    define_if_absent(env, &platform_loader, "emacs_jni/LispObject", LISP_OBJECT_CLASS)?;
    define_if_absent(env, &platform_loader, "emacs_jni/Lisp", LISP_CLASS)?;

    // Sanity check: confirm find_class can see what was just defined, the
    // same way every later task will look these classes up.
    env.find_class(jni::jni_str!("emacs_jni/LispCallable"))?;
    env.find_class(jni::jni_str!("emacs_jni/LispFunction"))?;
    env.find_class(jni::jni_str!("emacs_jni/LispError"))?;
    env.find_class(jni::jni_str!("emacs_jni/ReleaseNotifier"))?;
    env.find_class(jni::jni_str!("emacs_jni/LispRefReleaseNotifier"))?;
    env.find_class(jni::jni_str!("emacs_jni/LispObject"))?;
    env.find_class(jni::jni_str!("emacs_jni/Lisp"))?;

    register_lisp_function_native(env)?;
    register_release_notifier_native(env)?;
    register_lisp_native(env)?;
    register_lisp_object_native(env)?;
    register_lisp_ref_release_notifier_native(env)?;

    Ok(())
}

/// Registers the native implementation of `LispFunction.call` (Task 7's
/// half of "the full JVM-calls-Lisp round trip" — see `lisp_function_call`'s
/// own doc comment for what it actually does).
///
/// Uses jni-rs 0.22.4's `native_method!` macro (verified against the
/// vendored `jni-macros-0.22.4` crate source and
/// `jni-0.22.4/docs/macros/native_method.md`) to generate the ABI-correct
/// `extern "system"` wrapper together with the `NativeMethod` descriptor,
/// compile-time type-checked against `lisp_function_call`'s real
/// signature. This plan's original sketch (a hand-rolled
/// `extern "system" fn(env: jni::Env, ...) -> jni::sys::jobject` plus a
/// `NativeMethod { name, sig, fn_ptr }` struct literal) does not match
/// this crate version's actual API: `NativeMethod`'s fields are private,
/// constructible only via the `unsafe fn from_raw_parts` associated
/// function (not a struct literal), and a native method's real first
/// parameter must be `jni::EnvUnowned<'local>` (an as-yet-unattached
/// wrapper around the raw `JNIEnv` pointer the JVM hands a native method,
/// upgraded to a real `Env` via `.with_env(...)`), not a plain
/// `jni::Env<'local>` directly. `native_method!`'s non-`raw` mode builds
/// exactly that `EnvUnowned`-taking wrapper for us, along with panic
/// catching and `ThrowRuntimeExAndDefault` error handling (any
/// `jni::errors::Error` `lisp_function_call` returns becomes a thrown
/// `java.lang.RuntimeException`, synchronously, on the Java caller's own
/// `call()` invocation -- appropriate here, since a failure at this stage
/// means the round trip never even started, e.g. because posting to the
/// channel failed, as opposed to a failure that happens after a Lisp
/// callback runs, which instead completes the future exceptionally via
/// `LispError`, see `jvm_thread::handle_invoke_reply`).
fn register_lisp_function_native(env: &mut jni::Env) -> jni::errors::Result<()> {
    let class = env.find_class(jni::jni_str!("emacs_jni/LispFunction"))?;
    const CALL_METHOD: jni::NativeMethod = jni::native_method! {
        name = "call",
        fn lisp_function_call(args: [java.lang.Object]) -> java.util.concurrent.CompletableFuture,
    };
    // Safety: `native_method!` above compile-time-checks that
    // `lisp_function_call`'s Rust signature matches the JNI signature it
    // also encodes into `CALL_METHOD`, and `call` is a genuine non-static
    // instance method on `LispFunction` (matching `register_native_methods`'s
    // own extra safety requirement -- beyond what `NativeMethod` itself
    // checks -- that the second parameter's static/instance-ness agrees
    // with how the method is actually declared in Java; `native_method!`'s
    // generated wrapper also runs a runtime ABI check confirming this on
    // first call, so a mismatch here would fail loudly rather than
    // corrupt memory).
    unsafe { env.register_native_methods(&class, &[CALL_METHOD]) }
}

/// The actual implementation behind `LispFunction.call`: allocates a fresh
/// correlation id, constructs a (still-incomplete) `CompletableFuture`,
/// registers it in `pending`, posts `(invoke ID HANDLE ARGS)` on the
/// channel, and returns the future immediately — the concrete realization
/// of the design spec's "Java shim" section. Never blocks: everything here
/// is either process-local bookkeeping or a non-blocking channel write; the
/// actual Lisp call happens later, off this thread entirely (on Emacs's
/// own main thread, via `jvm--dispatch-message`'s `invoke` case), and its
/// result arrives back asynchronously via `Cmd::InvokeReply`
/// (`jvm_thread::handle_invoke_reply`).
///
/// Registered (not called directly) via `register_lisp_function_native`
/// above -- see that function's doc comment for the actual native-method
/// ABI this must satisfy, which `native_method!` enforces at compile time.
fn lisp_function_call<'local>(
    env: &mut jni::Env<'local>,
    this: JObject<'local>,
    args: JObjectArray<'local>,
) -> jni::errors::Result<JObject<'local>> {
    let handle = get_handle_field(env, &this)?;
    let id = crate::pending::next_id();

    let future_class = env.find_class(jni::jni_str!("java/util/concurrent/CompletableFuture"))?;
    let future = env.new_object(&future_class, jni::jni_sig!(() -> void), &[])?;
    let future_global = env.new_global_ref(&future)?;
    crate::pending::register(id, future_global);

    let args_sexp = jobject_array_to_sexp(env, &args)?;
    crate::channel::send(&crate::protocol::invoke_message(id, handle, &args_sexp));

    Ok(future)
}

/// Registers the native implementation of `ReleaseNotifier.run` — the
/// JVM-side mirror of `register_lisp_function_native` above, following the
/// exact same `native_method!` pattern (see that function's doc comment for
/// why this crate version needs the macro rather than a hand-rolled
/// `extern "system" fn` + manual `RegisterNatives` call). `run` takes no
/// arguments and returns `void`, per `Runnable#run`'s own signature.
fn register_release_notifier_native(env: &mut jni::Env) -> jni::errors::Result<()> {
    let class = env.find_class(jni::jni_str!("emacs_jni/ReleaseNotifier"))?;
    const RUN_METHOD: jni::NativeMethod = jni::native_method! {
        name = "run",
        fn release_notifier_run() -> void,
    };
    // Safety: same reasoning as `register_lisp_function_native`'s own
    // safety comment -- `native_method!` compile-time-checks
    // `release_notifier_run`'s signature against `RUN_METHOD`'s encoded
    // JNI signature, and `run` is a genuine non-static instance method on
    // `ReleaseNotifier`.
    unsafe { env.register_native_methods(&class, &[RUN_METHOD]) }
}

/// The actual implementation behind `ReleaseNotifier.run`: the opposite
/// direction from `jvm_thread`'s `Cmd::ReleaseRef` handling (Lisp's
/// `user-ptr` GC'd -> tell Rust to drop a `GlobalRef`, over the Lisp-to-
/// worker `Cmd` channel). Here, the JVM-side `LispFunction` proxy this
/// notifier was registered against (via `marshal::register_release_on_gc`)
/// was collected, so Lisp needs telling -- over the worker-to-Lisp
/// `channel::send` wire, not a `Cmd` -- to drop its own function-table
/// entry for the same handle. Runs on a `Cleaner`-managed background
/// thread, not the worker thread, but that's fine: `get_handle_field` is
/// plain JNI field access (needs only the `Env` this native call itself
/// provides) and `channel::send` just writes to a process-global pipe
/// writer, neither of which cares which thread calls them.
///
/// Registered (not called directly) via `register_release_notifier_native`
/// above.
fn release_notifier_run<'local>(env: &mut jni::Env<'local>, this: JObject<'local>) -> jni::errors::Result<()> {
    let handle = get_handle_field(env, &this)?;
    crate::channel::send(&crate::protocol::release_message(handle));
    Ok(())
}

/// Reads a private `handle` field directly via JNI field access, which
/// (unlike ordinary Java code) is not subject to Java-level access control
/// -- native code reading a private field of the very class that declares
/// the native method is exactly what JNI field access is for. Shared by
/// both `lisp_function_call` (THIS is a `LispFunction`) and
/// `release_notifier_run` (THIS is a `ReleaseNotifier`) -- both shim
/// classes declare their handle field identically (`private final long
/// handle`), so one reflective accessor covers both.
fn get_handle_field(env: &mut jni::Env<'_>, this: &JObject<'_>) -> jni::errors::Result<u64> {
    let value = env.get_field(this, jni::jni_str!("handle"), jni::jni_sig!(long))?.j()?;
    Ok(value as u64)
}

/// Renders an `Object[]` of `call`'s actual varargs arguments back into the
/// S-expression list fragment `protocol::invoke_message` embeds — the
/// reverse direction of what `jvm_thread::handle_call` (Task 5) does for
/// its own return values, reused here since a JVM-initiated call's
/// arguments need the exact same Java-object -> `LispArg` -> sexp pipeline
/// as any other value crossing from Java to Lisp.
fn jobject_array_to_sexp(env: &mut jni::Env<'_>, args: &JObjectArray<'_>) -> jni::errors::Result<String> {
    let len = args.len(env)?;
    let mut parts = Vec::with_capacity(len);
    for i in 0..len {
        let obj = args.get_element(env, i)?;
        let arg = crate::marshal::jvm_value_to_lisp_arg(env, JValueOwned::Object(obj))?;
        parts.push(crate::marshal::lisp_arg_to_sexp(&arg));
    }
    Ok(format!("({})", parts.join(" ")))
}

fn define_if_absent(
    env: &mut jni::Env,
    loader: &JClassLoader<'_>,
    name: &str,
    bytecode: &[u8],
) -> jni::errors::Result<()> {
    // `Env::define_class`'s real 0.22.4 signature takes `name: Option<N>
    // where N: AsRef<JNIStr>` and `loader: L where L: AsRef<JClassLoader>`
    // -- not the plain `name: &str, loader: &JObject` this task's brief
    // guessed. `JNIString::from(&str)` satisfies `AsRef<JNIStr>` (and so
    // does `&JNIString`, via std's blanket `AsRef<U> for &T where T:
    // AsRef<U>` -- reused below for the existence check).
    let jni_name = jni::strings::JNIString::from(name);
    // Skip re-defining a class the JVM already has -- see this function's
    // caller's doc comment on why `define_classes` must be idempotent
    // (the same underlying JVM can be reused across multiple `create_jvm`
    // calls within one process). `find_class` cleanly maps "not found" to
    // an `Err` without leaving a pending exception behind (verified
    // against the vendored 0.22.4 source, `env.rs`'s `find_class`), so
    // this is a safe existence probe.
    if env.find_class(&jni_name).is_ok() {
        return Ok(());
    }
    env.define_class(Some(&jni_name), loader, bytecode)?;
    Ok(())
}

/// How long a synchronous Lisp-access native (`intern`, `typeOf`,
/// `isNotNil`, `eq`, `LispObject.prin1ToString`) blocks its own reply
/// future before giving up — see design spec, "Risks": "a sync op blocks
/// its JVM thread until Emacs pumps... Mitigated by a JVM-side timeout."
const LISP_REQUEST_TIMEOUT_SECS: i64 = 30;

/// Registers the 5 static natives on `emacs_jni.Lisp` — the JVM-side entry
/// points for Clojure to hold and manipulate Lisp values (design spec
/// "Built-in Lisp access from Clojure"). Same `native_method!` pattern as
/// `register_lisp_function_native`; batched into one
/// `register_native_methods` call since they all belong to the same class.
fn register_lisp_native(env: &mut jni::Env) -> jni::errors::Result<()> {
    let class = env.find_class(jni::jni_str!("emacs_jni/Lisp"))?;
    const INTERN_METHOD: jni::NativeMethod = jni::native_method! {
        name = "intern",
        static fn lisp_intern(name: java.lang.String) -> emacs_jni.LispObject,
    };
    const TYPE_OF_METHOD: jni::NativeMethod = jni::native_method! {
        name = "typeOf",
        static fn lisp_type_of(v: emacs_jni.LispObject) -> emacs_jni.LispObject,
    };
    const IS_NOT_NIL_METHOD: jni::NativeMethod = jni::native_method! {
        name = "isNotNil",
        static fn lisp_is_not_nil(v: emacs_jni.LispObject) -> boolean,
    };
    const EQ_METHOD: jni::NativeMethod = jni::native_method! {
        name = "eq",
        static fn lisp_eq(a: emacs_jni.LispObject, b: emacs_jni.LispObject) -> boolean,
    };
    const APPLY_ASYNC_METHOD: jni::NativeMethod = jni::native_method! {
        name = "applyAsync",
        static fn lisp_apply_async(f: emacs_jni.LispObject, args: [java.lang.Object]) -> java.util.concurrent.CompletableFuture,
    };
    unsafe {
        env.register_native_methods(
            &class,
            &[INTERN_METHOD, TYPE_OF_METHOD, IS_NOT_NIL_METHOD, EQ_METHOD, APPLY_ASYNC_METHOD],
        )
    }
}

/// Registers `LispObject.prin1ToString`'s native implementation.
fn register_lisp_object_native(env: &mut jni::Env) -> jni::errors::Result<()> {
    let class = env.find_class(jni::jni_str!("emacs_jni/LispObject"))?;
    const PRIN1_TO_STRING_METHOD: jni::NativeMethod = jni::native_method! {
        name = "prin1ToString",
        fn lisp_object_prin1_to_string() -> java.lang.String,
    };
    unsafe { env.register_native_methods(&class, &[PRIN1_TO_STRING_METHOD]) }
}

/// Registers `LispRefReleaseNotifier.run`'s native implementation — the
/// Lisp-access counterpart of `register_release_notifier_native`.
fn register_lisp_ref_release_notifier_native(env: &mut jni::Env) -> jni::errors::Result<()> {
    let class = env.find_class(jni::jni_str!("emacs_jni/LispRefReleaseNotifier"))?;
    const RUN_METHOD: jni::NativeMethod = jni::native_method! {
        name = "run",
        fn lisp_ref_release_notifier_run() -> void,
    };
    unsafe { env.register_native_methods(&class, &[RUN_METHOD]) }
}

/// Common plumbing for every synchronous Lisp-access native: allocates a
/// correlation id, registers a fresh `CompletableFuture` in `pending`,
/// hands the request BUILD_REQUEST constructs (given that id) to
/// `lisp_requests::enqueue`, wakes the main thread up, then blocks this
/// JVM thread on the future's own `get(timeout, SECONDS)` — see design
/// spec, "Request/reply flow", step 1's "sync variant: `future.get()`
/// (blocks the JVM thread, with timeout)". Mirrors `lisp_function_call`'s
/// future/`pending`/`channel::send` setup, but waits in place instead of
/// handing the future back to the Java caller.
fn service_lisp_request_sync<'local>(
    env: &mut jni::Env<'local>,
    build_request: impl FnOnce(u64) -> crate::lisp_requests::LispRequest,
) -> jni::errors::Result<JObject<'local>> {
    let id = crate::pending::next_id();
    let future_class = env.find_class(jni::jni_str!("java/util/concurrent/CompletableFuture"))?;
    let future = env.new_object(&future_class, jni::jni_sig!(() -> void), &[])?;
    let future_global = env.new_global_ref(&future)?;
    crate::pending::register(id, future_global);

    crate::lisp_requests::enqueue(build_request(id));
    crate::channel::send(&crate::protocol::lisp_request_message());

    block_on_lisp_reply(env, &future)
}

/// Blocks on FUTURE's own `get(LISP_REQUEST_TIMEOUT_SECS, SECONDS)`,
/// reflection-free (a direct `CompletableFuture` instance method call, not
/// `Method.invoke`) — so unlike `describe_pending_java_exception`'s
/// `InvocationTargetException` unwrapping in `jvm_thread.rs`, a checked
/// exception from `get()` (`ExecutionException`/`TimeoutException`/
/// `InterruptedException`) is itself the JVM's pending exception, not
/// wrapped a level deeper.
fn block_on_lisp_reply<'local>(
    env: &mut jni::Env<'local>,
    future: &JObject<'local>,
) -> jni::errors::Result<JObject<'local>> {
    let time_unit_class = env.find_class(jni::jni_str!("java/util/concurrent/TimeUnit"))?;
    let seconds = env
        .get_static_field(&time_unit_class, jni::jni_str!("SECONDS"), jni::jni_sig!(java.util.concurrent.TimeUnit))?
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

/// Unwraps and rethrows the pending exception left by a failed
/// `CompletableFuture#get` call above: an `ExecutionException` wraps
/// whatever `jvm_thread::handle_lisp_reply` completed the future with
/// exceptionally (a `LispError`) — the useful part is that cause, one
/// `getCause()` hop down, same reasoning as
/// `jvm_thread::describe_pending_java_exception`. A `TimeoutException`/
/// `InterruptedException` (from `get`'s own timeout/interruption, not a
/// completed-exceptionally future) has no cause, so it's rethrown as-is.
///
/// Deliberately calls `env.throw` and returns its `Err(Error::JavaException)`
/// rather than building a Rust-level error message: `ThrowRuntimeExAndDefault`
/// (this crate's error policy for every native here) only throws a *new*
/// `RuntimeException` when no exception is already pending, so leaving the
/// real exception pending here is what lets it surface to the Java caller
/// unchanged instead of being wrapped a second time.
fn rethrow_lisp_reply_exception(env: &mut jni::Env) -> jni::errors::Error {
    let caught = match env.exception_catch() {
        Err(e) => e,
        Ok(()) => {
            return crate::marshal::marshal_error(
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
        Ok(()) => crate::marshal::marshal_error("env.throw unexpectedly left no pending exception"),
    }
}

/// `Lisp.intern`'s native implementation: `env.intern(name)` on the main
/// thread, via a `LispRequest::Intern` round trip.
fn lisp_intern<'local>(
    env: &mut jni::Env<'local>,
    _class: JClass<'local>,
    name: JString<'local>,
) -> jni::errors::Result<JObject<'local>> {
    let name = name.try_to_string(env)?;
    service_lisp_request_sync(env, |id| crate::lisp_requests::LispRequest::Intern { id, name })
}

/// `Lisp.typeOf`'s native implementation.
fn lisp_type_of<'local>(
    env: &mut jni::Env<'local>,
    _class: JClass<'local>,
    v: JObject<'local>,
) -> jni::errors::Result<JObject<'local>> {
    let ptr = get_ptr_field(env, &v)?;
    service_lisp_request_sync(env, |id| crate::lisp_requests::LispRequest::TypeOf { id, ptr })
}

/// `Lisp.isNotNil`'s native implementation.
fn lisp_is_not_nil<'local>(env: &mut jni::Env<'local>, _class: JClass<'local>, v: JObject<'local>) -> jni::errors::Result<bool> {
    let ptr = get_ptr_field(env, &v)?;
    let result = service_lisp_request_sync(env, |id| crate::lisp_requests::LispRequest::IsNotNil { id, ptr })?;
    unbox_boolean(env, &result)
}

/// `Lisp.eq`'s native implementation.
fn lisp_eq<'local>(
    env: &mut jni::Env<'local>,
    _class: JClass<'local>,
    a: JObject<'local>,
    b: JObject<'local>,
) -> jni::errors::Result<bool> {
    let a_ptr = get_ptr_field(env, &a)?;
    let b_ptr = get_ptr_field(env, &b)?;
    let result = service_lisp_request_sync(env, |id| crate::lisp_requests::LispRequest::Eq { id, a_ptr, b_ptr })?;
    unbox_boolean(env, &result)
}

/// `Lisp.applyAsync`'s native implementation: unlike the sync natives
/// above, never blocks -- enqueues the `Apply` request and returns the
/// still-incomplete future immediately (mirrors `lisp_function_call`).
fn lisp_apply_async<'local>(
    env: &mut jni::Env<'local>,
    _class: JClass<'local>,
    f: JObject<'local>,
    args: JObjectArray<'local>,
) -> jni::errors::Result<JObject<'local>> {
    let f_ptr = get_ptr_field(env, &f)?;
    let request_args = jobject_array_to_lisp_request_args(env, &args)?;

    let id = crate::pending::next_id();
    let future_class = env.find_class(jni::jni_str!("java/util/concurrent/CompletableFuture"))?;
    let future = env.new_object(&future_class, jni::jni_sig!(() -> void), &[])?;
    let future_global = env.new_global_ref(&future)?;
    crate::pending::register(id, future_global);

    crate::lisp_requests::enqueue(crate::lisp_requests::LispRequest::Apply { id, f_ptr, args: request_args });
    crate::channel::send(&crate::protocol::lisp_request_message());
    Ok(future)
}

/// `LispObject.prin1ToString`'s native implementation.
fn lisp_object_prin1_to_string<'local>(
    env: &mut jni::Env<'local>,
    this: JObject<'local>,
) -> jni::errors::Result<JString<'local>> {
    let ptr = get_ptr_field(env, &this)?;
    let result = service_lisp_request_sync(env, |id| crate::lisp_requests::LispRequest::Prin1 { id, ptr })?;
    env.cast_local::<JString>(result)
}

/// `LispRefReleaseNotifier.run`'s native implementation: enqueues a
/// `FreeRef` for this notifier's own `ptr` and wakes the main thread up.
/// No reply -- see `lisp_requests::LispRequest::FreeRef`'s own doc
/// comment ("carries no `id` and produces no reply").
fn lisp_ref_release_notifier_run<'local>(env: &mut jni::Env<'local>, this: JObject<'local>) -> jni::errors::Result<()> {
    let ptr = get_ptr_field(env, &this)?;
    crate::lisp_requests::enqueue(crate::lisp_requests::LispRequest::FreeRef { ptr });
    crate::channel::send(&crate::protocol::lisp_request_message());
    Ok(())
}

/// Unboxes a `java.lang.Boolean` OBJ (as returned by a sync Lisp-access
/// request's reply, built by `jvm_thread::build_lisp_reply_object`) into a
/// Rust `bool`.
fn unbox_boolean(env: &mut jni::Env, obj: &JObject) -> jni::errors::Result<bool> {
    env.call_method(obj, jni::jni_str!("booleanValue"), jni::jni_sig!(() -> boolean), &[])?.z()
}

/// Reads THIS's private `ptr` field — the `LispObject` analogue of
/// `get_handle_field`, same "native code reading its own declaring
/// class's private field" JNI access-control exemption.
fn get_ptr_field(env: &mut jni::Env<'_>, this: &JObject<'_>) -> jni::errors::Result<i64> {
    env.get_field(this, jni::jni_str!("ptr"), jni::jni_sig!(long))?.j()
}

/// Converts `applyAsync`'s `Object[]` ARGS into `LispRequestArg`s, per
/// spec "Argument conversion": a `LispObject` element carries by `ptr`;
/// everything else (including `null`) goes through the same
/// `marshal::jvm_value_to_lisp_arg` an ordinary `jvm-call` argument would.
fn jobject_array_to_lisp_request_args<'local>(
    env: &mut jni::Env<'local>,
    args: &JObjectArray<'local>,
) -> jni::errors::Result<Vec<crate::lisp_requests::LispRequestArg>> {
    let lisp_object_class = env.find_class(jni::jni_str!("emacs_jni/LispObject"))?;
    let len = args.len(env)?;
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let elem = args.get_element(env, i)?;
        let arg = if elem.is_null() {
            // `IsInstanceOf` (what `is_instance_of` calls) reports `true`
            // for a null reference against ANY class per the JNI spec, so
            // null must be checked before the `LispObject` check below --
            // otherwise a null argument would be misread as `Obj(0)`.
            crate::lisp_requests::LispRequestArg::Scalar(crate::commands::LispArg::Nil)
        } else if env.is_instance_of(&elem, &lisp_object_class)? {
            crate::lisp_requests::LispRequestArg::Obj(get_ptr_field(env, &elem)?)
        } else {
            let lisp_arg = crate::marshal::jvm_value_to_lisp_arg(env, JValueOwned::Object(elem))?;
            crate::lisp_requests::LispRequestArg::Scalar(lisp_arg)
        };
        out.push(arg);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jni::vm::{InitArgsBuilder, JavaVM};
    use jni::JNIVersion;

    /// Retries against jni-rs 0.22.4's racy check-then-create singleton --
    /// this file now has two tests each creating a JVM, so it needs the
    /// same retry loop `handles.rs`/`marshal.rs`/`jvm_thread.rs` already
    /// use (see their own copies' doc comments for the full explanation).
    fn create_test_jvm() -> JavaVM {
        let mut last_err = None;
        for _ in 0..20 {
            let init_args = InitArgsBuilder::new().version(JNIVersion::V1_8).build().unwrap();
            match JavaVM::new(init_args) {
                Ok(vm) => return vm,
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
        panic!("failed to create/attach a test JVM after retries: {last_err:?}");
    }

    #[test]
    fn defines_all_seven_shim_classes_and_they_are_findable() {
        let vm = create_test_jvm();
        vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            // `define_classes` itself already `find_class`es all seven
            // shim classes as its own internal sanity check, so this `?`
            // alone would already fail the test if any one of them (or
            // its native registration) were broken -- the extra
            // assignability checks below go further, confirming actual
            // interface relationships, not just "the class exists".
            define_classes(env)?;
            // Confirm LispFunction really implements LispCallable, not
            // just that both classes independently exist.
            let lisp_function_class = env.find_class(jni::jni_str!("emacs_jni/LispFunction"))?;
            let lisp_callable_class = env.find_class(jni::jni_str!("emacs_jni/LispCallable"))?;
            let is_assignable = env.is_assignable_from(&lisp_function_class, &lisp_callable_class)?;
            assert!(is_assignable, "LispFunction should implement LispCallable");
            // Same shape of check for ReleaseNotifier/LispRefReleaseNotifier:
            // confirm both really implement Runnable.
            let runnable_class = env.find_class(jni::jni_str!("java/lang/Runnable"))?;
            let release_notifier_class = env.find_class(jni::jni_str!("emacs_jni/ReleaseNotifier"))?;
            assert!(
                env.is_assignable_from(&release_notifier_class, &runnable_class)?,
                "ReleaseNotifier should implement Runnable"
            );
            let lisp_ref_release_notifier_class = env.find_class(jni::jni_str!("emacs_jni/LispRefReleaseNotifier"))?;
            assert!(
                env.is_assignable_from(&lisp_ref_release_notifier_class, &runnable_class)?,
                "LispRefReleaseNotifier should implement Runnable"
            );
            Ok(())
        })
        .expect("defining shim classes into a fresh JVM should succeed");
    }

    /// End-to-end proof that `intern`/`isNotNil`/`eq`/`typeOf`/`applyAsync`/
    /// `prin1ToString` actually work against a *real* Lisp-like `Env` --
    /// but this crate's unit tests have no real Emacs process to service
    /// `(lisp-request)`, so this test instead drives `lisp_requests::service`
    /// directly against a throwaway in-process "Env" substitute... which
    /// this crate cannot construct outside of a real Emacs module call
    /// (`emacs::Env` has no public constructor). The Lisp-access round
    /// trip is instead covered end-to-end by `test/jvm-clj-test.el`
    /// (a real `emacs --batch` process), and the pointer-discipline/
    /// FIFO-ordering pieces are covered in isolation by
    /// `lisp_requests`'s own unit tests. This comment documents that
    /// omission rather than fake an assertion.
    #[test]
    fn get_ptr_field_reads_back_what_was_stored() {
        let vm = create_test_jvm();
        vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            define_classes(env)?;
            let lisp_object_class = env.find_class(jni::jni_str!("emacs_jni/LispObject"))?;
            // `LispObject`'s constructor is package-private, but JNI
            // construction (unlike ordinary Java code) isn't subject to
            // that -- same reasoning as `get_handle_field`'s doc comment.
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
