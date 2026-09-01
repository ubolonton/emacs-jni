use std::sync::mpsc::Receiver;

use jni::vm::{InitArgsBuilder, JavaVM};
use jni::JNIVersion;

use crate::commands::Cmd;
use crate::protocol;
use crate::status::{Status, STATUS};
use crate::{channel, worker};

/// Entry point for the JVM worker thread. Runs for the JVM's entire
/// lifetime: creates the JVM once, then loops handling commands until a
/// successful `Cmd::Stop` (or, from Task 8 on, an unrecoverable panic).
pub fn run(receiver: Receiver<Cmd>, classpath: Vec<String>, options: Vec<String>) {
    let vm = match create_jvm(&classpath, &options) {
        Ok(vm) => {
            STATUS.finish(Status::Ready);
            channel::send(&protocol::status_message(Status::Ready, None));
            vm
        }
        Err(e) => {
            STATUS.finish(Status::StartFailed);
            channel::send(&protocol::status_message(Status::StartFailed, Some(&e)));
            worker::clear_sender();
            return;
        }
    };

    // Lazily created on the first LoadJar command, then reused so every
    // loaded jar shares one classloader (see spec: "Jar loading").
    let mut loader: Option<jni::refs::Global<jni::objects::JObject<'static>>> = None;

    for cmd in receiver {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match cmd {
            Cmd::Resume => {
                handle_resume(&vm);
                false
            }
            Cmd::Pause => {
                handle_pause(&vm);
                false
            }
            Cmd::Stop => true, // signal "stop after this iteration" below
            Cmd::LoadJar { id, path } => {
                handle_load_jar(&vm, &mut loader, id, path);
                false
            }
            #[cfg(feature = "test-support")]
            Cmd::TestPanic => panic!("jvm--test-panic: deliberate panic for crash-handling tests"),
        }));

        match outcome {
            Ok(true) => {
                // Cmd::Stop: `vm` needs to move into handle_stop, which
                // catch_unwind's closure can't do (AssertUnwindSafe wraps
                // a Fn-like closure, not a one-shot move of `vm` itself).
                // Call it here, outside the closure, then break.
                handle_stop(vm);
                break;
            }
            Ok(false) => {}
            Err(payload) => {
                let message = panic_message(&payload);
                STATUS.finish(Status::Crashed);
                channel::send(&protocol::status_message(Status::Crashed, Some(&message)));
                worker::clear_sender();
                return;
            }
        }
    }
}

fn create_jvm(classpath: &[String], options: &[String]) -> Result<JavaVM, String> {
    let mut builder = InitArgsBuilder::new().version(JNIVersion::V1_8);
    if !classpath.is_empty() {
        builder = builder.option(format!("-Djava.class.path={}", classpath.join(":")));
    }
    for opt in options {
        builder = builder.option(opt.clone());
    }
    let init_args = builder.build().map_err(|e| e.to_string())?;
    JavaVM::new(init_args).map_err(|e| e.to_string())
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

fn handle_pause(vm: &JavaVM) {
    match vm.detach_current_thread() {
        Ok(()) => {
            STATUS.finish(Status::Paused);
            channel::send(&protocol::status_message(Status::Paused, None));
        }
        Err(e) => {
            // Detach failing is unexpected; stay in `pausing` rather than
            // claiming success we can't back up, and surface it as a log
            // line rather than a status (there's no `pause-failed` state
            // in the spec's state machine, so pausing simply never
            // completes — the caller's `-sync` wrapper times out).
            channel::send(&protocol::log_message(
                "error",
                &format!("jvm-pause: detach_current_thread failed: {e}"),
            ));
        }
    }
}

fn handle_stop(vm: JavaVM) {
    // Safety: `destroy` requires no Java methods on the calling thread's
    // stack (true here — we're between command handlers) and that the
    // JavaVM isn't used again afterward except to drop it, which is
    // exactly what follows.
    let result = unsafe { vm.destroy() };
    std::mem::drop(vm);
    match result {
        Ok(()) => {
            STATUS.finish(Status::Stopped);
            channel::send(&protocol::status_message(Status::Stopped, None));
        }
        Err(e) => {
            STATUS.finish(Status::StopFailed);
            channel::send(&protocol::status_message(Status::StopFailed, Some(&e.to_string())));
        }
    }
    worker::clear_sender();
}

fn handle_resume(vm: &JavaVM) {
    // Cheap: attach_current_thread is a fast thread-local check if the
    // thread is already attached, and a real (re-)attach if `Cmd::Pause`
    // (Task 5) had explicitly detached it.
    let result: jni::errors::Result<()> = vm.attach_current_thread(|_env| Ok(()));
    match result {
        Ok(()) => {
            STATUS.finish(Status::Ready);
            channel::send(&protocol::status_message(Status::Ready, None));
        }
        Err(e) => {
            STATUS.finish(Status::StartFailed);
            channel::send(&protocol::status_message(Status::StartFailed, Some(&e.to_string())));
        }
    }
}

fn handle_load_jar(
    vm: &JavaVM,
    loader: &mut Option<jni::refs::Global<jni::objects::JObject<'static>>>,
    id: u64,
    path: String,
) {
    // `URLClassLoader#addURL` is lazy: it just appends to the loader's
    // internal URL list and never inspects the target until a class is
    // actually looked up, so it does not itself fail for a nonexistent
    // path (verified empirically -- see Task 7's report). Check existence
    // up front so `jvm-load-jar`'s ok/err result actually reflects whether
    // the jar is there, without touching the reflective addURL sequence
    // below.
    if !std::path::Path::new(&path).is_file() {
        channel::send(&protocol::load_jar_done_message(
            id,
            false,
            Some(&format!("jar file not found: {path}")),
        ));
        return;
    }

    let result: jni::errors::Result<()> = vm.attach_current_thread(|env| {
        if loader.is_none() {
            *loader = Some(create_shared_classloader(env)?);
        }
        let loader_local = env.new_local_ref(loader.as_ref().unwrap().as_obj())?;
        add_jar_url(env, &loader_local, &path)
    });

    match result {
        Ok(()) => channel::send(&protocol::load_jar_done_message(id, true, None)),
        Err(e) => channel::send(&protocol::load_jar_done_message(id, false, Some(&e.to_string()))),
    }
}

/// Creates a `URLClassLoader` with an empty URL list, parented on the
/// platform classloader (not the application classloader) so jars we load
/// don't inherit whatever's already on this module's own classpath.
fn create_shared_classloader<'local>(
    env: &mut jni::Env<'local>,
) -> jni::errors::Result<jni::refs::Global<jni::objects::JObject<'static>>> {
    let class_loader_class = env.find_class(jni::jni_str!("java/lang/ClassLoader"))?;
    let platform_loader = env
        .call_static_method(
            &class_loader_class,
            jni::jni_str!("getPlatformClassLoader"),
            jni::jni_sig!(() -> java.lang.ClassLoader),
            &[],
        )?
        .l()?;

    let url_class = env.find_class(jni::jni_str!("java/net/URL"))?;
    let empty_urls = env.new_object_array(0, &url_class, jni::objects::JObject::null())?;

    let loader_class = env.find_class(jni::jni_str!("java/net/URLClassLoader"))?;
    let loader = env.new_object(
        &loader_class,
        jni::jni_sig!((urls: [java.net.URL], parent: java.lang.ClassLoader) -> void),
        &[
            jni::objects::JValue::Object(&empty_urls),
            jni::objects::JValue::Object(&platform_loader),
        ],
    )?;

    env.new_global_ref(&loader)
}

/// Converts PATH to a `file:` URL and adds it to LOADER via the protected
/// `URLClassLoader#addURL(URL)` method, called reflectively (Java gives no
/// public API to extend an existing URLClassLoader's URL list).
fn add_jar_url(
    env: &mut jni::Env<'_>,
    loader: &jni::objects::JObject<'_>,
    path: &str,
) -> jni::errors::Result<()> {
    let path_str = env.new_string(path)?;
    let file_class = env.find_class(jni::jni_str!("java/io/File"))?;
    let file = env.new_object(
        &file_class,
        jni::jni_sig!((pathname: java.lang.String) -> void),
        &[jni::objects::JValue::Object(&path_str.into())],
    )?;
    let uri = env
        .call_method(&file, jni::jni_str!("toURI"), jni::jni_sig!(() -> java.net.URI), &[])?
        .l()?;
    let url = env
        .call_method(&uri, jni::jni_str!("toURL"), jni::jni_sig!(() -> java.net.URL), &[])?
        .l()?;

    let url_class = env.find_class(jni::jni_str!("java/net/URL"))?;
    let loader_class = env.find_class(jni::jni_str!("java/net/URLClassLoader"))?;
    let class_class = env.find_class(jni::jni_str!("java/lang/Class"))?;
    let param_types = env.new_object_array(1, &class_class, &url_class)?;

    let method_name = env.new_string("addURL")?;
    let method = env
        .call_method(
            &loader_class,
            jni::jni_str!("getDeclaredMethod"),
            jni::jni_sig!((name: java.lang.String, parameterTypes: [java.lang.Class]) -> java.lang.reflect.Method),
            &[
                jni::objects::JValue::Object(&method_name.into()),
                jni::objects::JValue::Object(&param_types),
            ],
        )?
        .l()?;

    env.call_method(
        &method,
        jni::jni_str!("setAccessible"),
        jni::jni_sig!((flag: boolean) -> void),
        &[jni::objects::JValue::Bool(true)],
    )?;

    let object_class = env.find_class(jni::jni_str!("java/lang/Object"))?;
    let invoke_args = env.new_object_array(1, &object_class, &url)?;
    env.call_method(
        &method,
        jni::jni_str!("invoke"),
        jni::jni_sig!((obj: java.lang.Object, args: [java.lang.Object]) -> java.lang.Object),
        &[
            jni::objects::JValue::Object(loader),
            jni::objects::JValue::Object(&invoke_args),
        ],
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    /// Compiles and jars up `test/fixtures/Greeter.java` into
    /// `target/test-fixtures/greeter.jar`, mirroring what
    /// `test/run-ert-tests.sh` does for the ERT suite. Doing it here too
    /// keeps this test self-sufficient under a plain `cargo test`, with no
    /// dependency on the shell script having run first.
    fn build_greeter_jar() -> PathBuf {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let out_dir = manifest_dir.join("target/test-fixtures");
        std::fs::create_dir_all(&out_dir).expect("failed to create target/test-fixtures");
        let source = manifest_dir.join("test/fixtures/Greeter.java");

        let status = Command::new("javac")
            .arg("-d")
            .arg(&out_dir)
            .arg(&source)
            .status()
            .expect("failed to run javac (is a JDK on PATH?)");
        assert!(status.success(), "javac failed to compile {source:?}");

        let jar_path = out_dir.join("greeter.jar");
        let status = Command::new("jar")
            .arg("cf")
            .arg(&jar_path)
            .arg("-C")
            .arg(&out_dir)
            .arg("Greeter.class")
            .status()
            .expect("failed to run jar (is a JDK on PATH?)");
        assert!(status.success(), "jar failed to package Greeter.class into {jar_path:?}");

        jar_path
    }

    /// This is the load-bearing proof for Task 7: it doesn't just check that
    /// `add_jar_url` returns `Ok(())` (which `addURL` would do even for a
    /// path that isn't a real jar, since `URLClassLoader` only inspects a
    /// URL's contents lazily on first class load) -- it reflectively loads
    /// `Greeter` through the very `URLClassLoader` `add_jar_url` extended,
    /// then calls `Greeter.greet()` and checks the actual returned string.
    /// That's only possible if the shared classloader's URL list genuinely
    /// now includes the fixture jar.
    #[test]
    fn add_jar_url_makes_the_jars_class_loadable_and_callable() {
        let jar_path = build_greeter_jar();
        let vm = create_jvm(&[], &[]).expect("failed to create JVM");

        let greeting: String = vm
            .attach_current_thread(|env| -> jni::errors::Result<String> {
                let loader_global = create_shared_classloader(env)?;
                let loader_local = env.new_local_ref(loader_global.as_obj())?;
                add_jar_url(env, &loader_local, jar_path.to_str().unwrap())?;

                let name = env.new_string("Greeter")?;
                let class_obj = env
                    .call_method(
                        &loader_local,
                        jni::jni_str!("loadClass"),
                        jni::jni_sig!((name: java.lang.String) -> java.lang.Class),
                        &[jni::objects::JValue::Object(&name.into())],
                    )?
                    .l()?;
                let greeter_class = env.cast_local::<jni::objects::JClass>(class_obj)?;

                let result = env
                    .call_static_method(
                        &greeter_class,
                        jni::jni_str!("greet"),
                        jni::jni_sig!(() -> java.lang.String),
                        &[],
                    )?
                    .l()?;
                let result = env.cast_local::<jni::objects::JString>(result)?;
                Ok(result.to_string())
            })
            .expect("reflective load-then-invoke of Greeter.greet() failed");

        assert_eq!(greeting, "hello from Greeter");
    }
}
