use jni::objects::JClassLoader;

const LISP_CALLABLE_CLASS: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/target/emacs-jni-shim-classes/emacs_jni/LispCallable.class"));
const LISP_FUNCTION_CLASS: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/target/emacs-jni-shim-classes/emacs_jni/LispFunction.class"));
const LISP_ERROR_CLASS: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/target/emacs-jni-shim-classes/emacs_jni/LispError.class"));

/// Defines the `emacs_jni.{LispCallable,LispFunction,LispError}` shim
/// classes into the currently-attached JVM, using the *platform* classloader
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
    // LispError has no dependency and can go anywhere.
    define_if_absent(env, &platform_loader, "emacs_jni/LispCallable", LISP_CALLABLE_CLASS)?;
    define_if_absent(env, &platform_loader, "emacs_jni/LispFunction", LISP_FUNCTION_CLASS)?;
    define_if_absent(env, &platform_loader, "emacs_jni/LispError", LISP_ERROR_CLASS)?;

    // Sanity check: confirm find_class can see what was just defined, the
    // same way every later task will look these classes up.
    env.find_class(jni::jni_str!("emacs_jni/LispCallable"))?;
    env.find_class(jni::jni_str!("emacs_jni/LispFunction"))?;
    env.find_class(jni::jni_str!("emacs_jni/LispError"))?;

    Ok(())
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
    fn defines_all_three_shim_classes_and_they_are_findable() {
        let vm = create_test_jvm();
        vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            define_classes(env)?;
            // Confirm LispFunction really implements LispCallable, not
            // just that both classes independently exist.
            let lisp_function_class = env.find_class(jni::jni_str!("emacs_jni/LispFunction"))?;
            let lisp_callable_class = env.find_class(jni::jni_str!("emacs_jni/LispCallable"))?;
            let is_assignable = env.is_assignable_from(&lisp_function_class, &lisp_callable_class)?;
            assert!(is_assignable, "LispFunction should implement LispCallable");
            Ok(())
        })
        .expect("defining shim classes into a fresh JVM should succeed");
    }
}
