use jni::objects::{JClassLoader, JObject, JObjectArray, JValueOwned};

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

/// Defines the `emacs_jni.{LispCallable,LispFunction,LispError,
/// ReleaseNotifier}` shim classes into the currently-attached JVM, using
/// the *platform* classloader
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

    // Sanity check: confirm find_class can see what was just defined, the
    // same way every later task will look these classes up.
    env.find_class(jni::jni_str!("emacs_jni/LispCallable"))?;
    env.find_class(jni::jni_str!("emacs_jni/LispFunction"))?;
    env.find_class(jni::jni_str!("emacs_jni/LispError"))?;
    env.find_class(jni::jni_str!("emacs_jni/ReleaseNotifier"))?;

    register_lisp_function_native(env)?;
    register_release_notifier_native(env)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use jni::vm::{InitArgsBuilder, JavaVM};
    use jni::JNIVersion;

    fn create_test_jvm() -> JavaVM {
        let init_args = InitArgsBuilder::new().version(JNIVersion::V1_8).build().unwrap();
        JavaVM::new(init_args).unwrap()
    }

    #[test]
    fn defines_all_four_shim_classes_and_they_are_findable() {
        let vm = create_test_jvm();
        vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            // `define_classes` itself already `find_class`es all four shim
            // classes as its own internal sanity check, so this `?` alone
            // would already fail the test if `ReleaseNotifier` (or its
            // native `run` registration) were broken -- the extra
            // assignability checks below go further, confirming actual
            // interface relationships, not just "the class exists".
            define_classes(env)?;
            // Confirm LispFunction really implements LispCallable, not
            // just that both classes independently exist.
            let lisp_function_class = env.find_class(jni::jni_str!("emacs_jni/LispFunction"))?;
            let lisp_callable_class = env.find_class(jni::jni_str!("emacs_jni/LispCallable"))?;
            let is_assignable = env.is_assignable_from(&lisp_function_class, &lisp_callable_class)?;
            assert!(is_assignable, "LispFunction should implement LispCallable");
            // Same shape of check for the new fourth class: confirm
            // ReleaseNotifier really implements Runnable.
            let release_notifier_class = env.find_class(jni::jni_str!("emacs_jni/ReleaseNotifier"))?;
            let runnable_class = env.find_class(jni::jni_str!("java/lang/Runnable"))?;
            let is_runnable = env.is_assignable_from(&release_notifier_class, &runnable_class)?;
            assert!(is_runnable, "ReleaseNotifier should implement Runnable");
            Ok(())
        })
        .expect("defining shim classes into a fresh JVM should succeed");
    }
}
