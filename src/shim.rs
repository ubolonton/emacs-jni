use jni::objects::{JClass, JClassLoader};

const LISP_CALLABLE_CLASS: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/target/emacs-jni-shim-classes/emacs_jni/LispCallable.class"));
const LISP_FUNCTION_CLASS: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/target/emacs-jni-shim-classes/emacs_jni/LispFunction.class"));
const LISP_ERROR_CLASS: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/target/emacs-jni-shim-classes/emacs_jni/LispError.class"));

/// Defines the `emacs_jni.{LispCallable,LispFunction,LispError}` shim
/// classes into the currently-attached JVM, using the system classloader so
/// later `find_class` calls (made with no Java frames on the stack) can
/// find them again by name. Must be called once, right after the JVM is
/// created, before anything else in this crate references those classes.
pub fn define_classes(env: &mut jni::Env) -> jni::errors::Result<()> {
    // Matches `create_shared_classloader`'s reflective style in
    // jvm_thread.rs (find_class + call_static_method) rather than the
    // jni-rs 0.22.4 typed `JClassLoader::get_system_class_loader(env)`
    // convenience binding, to keep one JNI-calling style across the crate.
    let class_loader_class = env.find_class(jni::jni_str!("java/lang/ClassLoader"))?;
    let system_loader_obj = env
        .call_static_method(
            &class_loader_class,
            jni::jni_str!("getSystemClassLoader"),
            jni::jni_sig!(() -> java.lang.ClassLoader),
            &[],
        )?
        .l()?;
    let system_loader = env.cast_local::<JClassLoader>(system_loader_obj)?;

    // LispCallable must be defined before LispFunction (which implements
    // it) for the JVM's verifier to resolve the interface reference.
    // LispError has no dependency and can go anywhere.
    define_one(env, &system_loader, "emacs_jni/LispCallable", LISP_CALLABLE_CLASS)?;
    define_one(env, &system_loader, "emacs_jni/LispFunction", LISP_FUNCTION_CLASS)?;
    define_one(env, &system_loader, "emacs_jni/LispError", LISP_ERROR_CLASS)?;

    // Sanity check: confirm find_class can see what was just defined, the
    // same way every later task will look these classes up.
    env.find_class(jni::jni_str!("emacs_jni/LispCallable"))?;
    env.find_class(jni::jni_str!("emacs_jni/LispFunction"))?;
    env.find_class(jni::jni_str!("emacs_jni/LispError"))?;

    Ok(())
}

fn define_one<'local>(
    env: &mut jni::Env<'local>,
    loader: &JClassLoader<'_>,
    name: &str,
    bytecode: &[u8],
) -> jni::errors::Result<JClass<'local>> {
    // `Env::define_class`'s real 0.22.4 signature takes `name: Option<N>
    // where N: AsRef<JNIStr>` and `loader: L where L: AsRef<JClassLoader>`
    // -- not the plain `name: &str, loader: &JObject` this task's brief
    // guessed. `JNIString::from(&str)` satisfies `AsRef<JNIStr>`.
    env.define_class(Some(jni::strings::JNIString::from(name)), loader, bytecode)
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
