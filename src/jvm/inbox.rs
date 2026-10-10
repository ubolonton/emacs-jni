//! The worker thread: its `Cmd` queue, event loop, JVM lifecycle, jar loading, and the table of
//! futures waiting for a Lisp reply. Reflective calls are in `jvm::invoke`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use jni::objects::JObject;
use jni::refs::Global;
use jni::vm::{InitArgsBuilder, JavaVM};
use jni::JNIVersion;

use crate::jvm::convert::JvmObject;
use crate::jvm::runtime::STATUS;
use crate::protocol::{BridgeValue, CallTarget, LispOutcome, Status};

#[derive(Debug)]
pub(crate) enum Cmd {
    /// Resumes a paused worker. A first start spawns a new thread instead (`start_or_resume`).
    Resume,
    Pause,
    Stop,
    LoadJar {
        id: u64,
        path: String,
    },
    Call {
        id: u64,
        target: CallTarget,
        member: String,
        args: Vec<BridgeValue>,
    },
    /// Completes a `LispFunction.call` future with the Lisp callback's outcome. Sent by
    /// `jvm--invoke-reply`.
    InvokeReply {
        id: u64,
        outcome: LispOutcome,
    },
    /// Drops a JVM reference whose Lisp `user-ptr` was collected. Sent by `JvmHandle::drop`, so
    /// the delete runs on an attached thread.
    ReleaseRef {
        object: JvmObject,
    },
    /// Completes a Lisp-access native's future (`intern`, `applyAsync`, ...). Sent after the main
    /// thread runs the request.
    LispReply {
        id: u64,
        value: LispOutcome,
    },
    #[cfg(feature = "test-support")]
    TestPanic,
}

static SENDER: Mutex<Option<Sender<Cmd>>> = Mutex::new(None);
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_request_id() -> u64 {
    NEXT_REQUEST_ID.fetch_add(1, Ordering::SeqCst)
}

/// Resumes the worker if it is alive (paused), or spawns a new one.
pub(crate) fn start_or_resume(classpath: Vec<String>, options: Vec<String>) {
    let mut guard = SENDER.lock().unwrap();
    if let Some(sender) = guard.as_ref() {
        let _ = sender.send(Cmd::Resume);
    } else {
        let (tx, rx) = mpsc::channel();
        *guard = Some(tx);
        thread::spawn(move || run(rx, classpath, options));
    }
}

/// Sends a command other than `Resume` to the worker. Returns `Err(())` if no worker is running.
pub(crate) fn submit(cmd: Cmd) -> Result<(), ()> {
    let guard = SENDER.lock().unwrap();
    match guard.as_ref() {
        Some(sender) => sender.send(cmd).map_err(|_| ()),
        None => Err(()),
    }
}

/// Called by the worker just before it exits (stop or panic), so the next `jvm-start` spawns a
/// new thread.
pub(crate) fn clear_sender() {
    *SENDER.lock().unwrap() = None;
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
// A future has one owner at a time (this table, then `take`'s caller), so no `Arc`.
static TABLE: Mutex<Option<HashMap<u64, Global<JObject<'static>>>>> = Mutex::new(None);

fn table() -> std::sync::MutexGuard<'static, Option<HashMap<u64, Global<JObject<'static>>>>> {
    let mut guard = TABLE.lock().unwrap();
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    guard
}

/// Allocates a correlation id for an `invoke` round trip. The id is needed before the future
/// exists.
pub(crate) fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::SeqCst)
}

/// Registers FUTURE (a global ref to a `java.util.concurrent.CompletableFuture`)
/// under ID, for later completion by `Cmd::InvokeReply`.
pub(crate) fn register(id: u64, future: Global<JObject<'static>>) {
    table().as_mut().unwrap().insert(id, future);
}

/// Removes and returns the pending future for ID. Each id is completed once.
fn take(id: u64) -> Option<Global<JObject<'static>>> {
    table().as_mut().unwrap().remove(&id)
}

/// Entry point for the worker thread. Creates the JVM, then handles commands until a successful
/// `Cmd::Stop` or a panic.
pub(crate) fn run(receiver: Receiver<Cmd>, classpath: Vec<String>, options: Vec<String>) {
    let vm = match create_jvm(&classpath, &options) {
        Ok(vm) => {
            // Publish the VM for `call_direct` on the Emacs main thread.
            crate::jvm::runtime::set_vm(vm.clone());
            STATUS.finish(Status::Ready);
            crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::Status {
                status: Status::Ready,
                detail: None,
            });
            vm
        }
        Err(e) => {
            STATUS.finish(Status::StartFailed);
            crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::Status {
                status: Status::StartFailed,
                detail: Some(e),
            });
            clear_sender();
            return;
        }
    };

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
                handle_load_jar(&vm, id, path);
                false
            }
            Cmd::Call { id, target, member, args } => {
                // On its own thread: a Java call can be slow and must not block this loop.
                let vm = vm.clone();
                let loader = crate::jvm::runtime::loader();
                std::thread::spawn(move || {
                    crate::jvm::invoke::handle_call(&vm, loader, id, target, member, args)
                });
                false
            }
            Cmd::InvokeReply { id, outcome } => {
                // Inline: completing a future is cheap.
                handle_invoke_reply(&vm, id, outcome);
                false
            }
            // Inline: dropping the last `Arc` here runs `DeleteGlobalRef` on an attached thread.
            Cmd::ReleaseRef { object } => {
                std::mem::drop(object);
                false
            }
            // Inline: completing a future is cheap.
            Cmd::LispReply { id, value } => {
                handle_lisp_reply(&vm, id, value);
                false
            }
            #[cfg(feature = "test-support")]
            Cmd::TestPanic => panic!("jvm--test-panic: deliberate panic for crash-handling tests"),
        }));

        match outcome {
            Ok(true) => {
                // `handle_stop` takes `vm` by value, which the `catch_unwind` closure cannot.
                handle_stop(vm);
                break;
            }
            Ok(false) => {}
            Err(payload) => {
                let message = panic_message(&payload);
                STATUS.finish(Status::Crashed);
                crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::Status {
                    status: Status::Crashed,
                    detail: Some(message),
                });
                clear_sender();
                return;
            }
        }
    }
}

// Whether a `create_jvm` call has reached `define_classes` before in this process. Only the first
// such call treats a `define_classes` failure as fatal; see `create_jvm`.
//
// The flag is set just before `define_classes`, not at the start of `create_jvm`. A start can
// fail earlier (bad options) and be retried from `start-failed`; that retry is still the first
// real attempt. Worker threads never run `create_jvm` concurrently, so `swap` is enough.
static JVM_CREATE_ATTEMPTED: AtomicBool = AtomicBool::new(false);

/// Returns `true` on the first call for FLAG, `false` after. A function so that a test can use
/// its own flag.
fn is_first_attempt(flag: &AtomicBool) -> bool {
    !flag.swap(true, Ordering::SeqCst)
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
    let vm = JavaVM::new(init_args).map_err(|e| e.to_string())?;

    // A `define_classes` failure on the first attempt means the shim is broken: fail the start
    // (`start-failed`). On a later attempt (restart after `jvm-stop` or a crash) the classes are
    // usually already defined, and the failure means the destroyed JVM is unusable; any JNI work
    // fails the same way. Log it and report `ready`, as a restart always did.
    //
    // jni 0.22.4 caches the `JavaVM` in a process-global singleton, so a restart re-attaches to
    // the same JVM. See `docs/explanation/limitations-and-tradeoffs.md`.
    let is_first_attempt = is_first_attempt(&JVM_CREATE_ATTEMPTED);
    let result: jni::errors::Result<()> =
        vm.attach_current_thread(crate::jvm::entry::define_classes);
    match result {
        Ok(()) => {}
        Err(e) if is_first_attempt => return Err(e.to_string()),
        Err(e) => {
            crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::Log {
                level: "error".to_string(),
                text: format!("failed to define emacs_jni.core shim classes: {e}"),
            });
        }
    }

    Ok(vm)
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
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
            crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::Status {
                status: Status::Paused,
                detail: None,
            });
        }
        Err(e) => {
            // Stay in `pausing` and log. There is no `pause-failed` state; the caller's `-await`
            // wrapper times out.
            crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::Log {
                level: "error".to_string(),
                text: format!("jvm-pause: detach_current_thread failed: {e}"),
            });
        }
    }
}

fn handle_stop(vm: JavaVM) {
    // Safety: `destroy` requires no Java frames on this thread's stack (true between commands)
    // and no later use of `vm`.
    let result = unsafe { vm.destroy() };
    // The `jvm--stop` defun already detached the main thread, so it cannot block `destroy`.
    crate::jvm::runtime::clear();
    match result {
        Ok(()) => {
            STATUS.finish(Status::Stopped);
            crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::Status {
                status: Status::Stopped,
                detail: None,
            });
        }
        Err(e) => {
            STATUS.finish(Status::StopFailed);
            crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::Status {
                status: Status::StopFailed,
                detail: Some(e.to_string()),
            });
        }
    }
    clear_sender();
}

fn handle_resume(vm: &JavaVM) {
    // A no-op if attached; re-attaches if `Cmd::Pause` detached this thread.
    let result: jni::errors::Result<()> = vm.attach_current_thread(|_env| Ok(()));
    match result {
        Ok(()) => {
            STATUS.finish(Status::Ready);
            crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::Status {
                status: Status::Ready,
                detail: None,
            });
        }
        Err(e) => {
            STATUS.finish(Status::StartFailed);
            crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::Status {
                status: Status::StartFailed,
                detail: Some(e.to_string()),
            });
        }
    }
}

fn handle_load_jar(vm: &JavaVM, id: u64, path: String) {
    // `URLClassLoader#addURL` does not check the path, so check it here.
    //
    // A directory also works: `File::toURI` adds the trailing `/` that `URLClassLoader` needs.
    // `jvm-clj-start` uses this to add `clojure-src/`.
    if !std::path::Path::new(&path).exists() {
        crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::LoadJarDone {
            id,
            ok: false,
            err: Some(format!("jar/classpath entry not found: {path}")),
        });
        return;
    }

    let result: jni::errors::Result<()> = vm.attach_current_thread(|env| {
        let loader = crate::jvm::runtime::get_or_create_loader(|| {
            create_shared_classloader(env).map(Arc::new)
        })?;
        let loader_local = env.new_local_ref(loader.as_obj())?;
        add_jar_url(env, &loader_local, &path)
    });

    match result {
        Ok(()) => crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::LoadJarDone {
            id,
            ok: true,
            err: None,
        }),
        Err(e) => crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::LoadJarDone {
            id,
            ok: false,
            err: Some(e.to_string()),
        }),
    }
}

/// Synchronous counterpart of `handle_load_jar`, for `jvm--load-jar-direct`. Uses the same shared
/// classloader. Returns an error message instead of enqueueing `Inbound::LoadJarDone`.
pub(crate) fn load_jar_direct(vm: &JavaVM, path: &str) -> Result<(), String> {
    // `addURL` does not check the path.
    if !std::path::Path::new(path).exists() {
        return Err(format!("jar/classpath entry not found: {path}"));
    }
    // Scoped attach: `addURL` needs no stable thread identity.
    let result: jni::errors::Result<()> = vm.attach_current_thread_for_scope(|env| {
        let loader = crate::jvm::runtime::get_or_create_loader(|| {
            create_shared_classloader(env).map(Arc::new)
        })?;
        let loader_local = env.new_local_ref(loader.as_obj())?;
        add_jar_url(env, &loader_local, path)
    });
    result.map_err(|e| e.to_string())
}

/// Creates an empty `URLClassLoader` whose parent is the platform classloader, so loaded jars do
/// not see this module's own classpath.
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

/// Adds PATH to LOADER as a `file:` URL. `URLClassLoader#addURL` is protected, so it is called
/// reflectively.
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
        &[jni::objects::JValue::Object(loader), jni::objects::JValue::Object(&invoke_args)],
    )?;

    Ok(())
}

/// Completes the future for ID with a Lisp callback's OUTCOME. Inline on the worker, because
/// completing a future is cheap.
fn handle_invoke_reply(vm: &JavaVM, id: u64, outcome: crate::protocol::LispOutcome) {
    let Some(future) = take(id) else {
        crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::Log {
            level: "error".to_string(),
            text: format!("invoke-reply for unknown id {id}"),
        });
        return;
    };
    let result: jni::errors::Result<()> = vm.attach_current_thread(|env| {
        let future_local = env.new_local_ref(future.as_obj())?;
        crate::jvm::invoke::complete_future_with_reply(env, &future_local, outcome)
    });
    if let Err(e) = result {
        // The future is already out of the table, so the Java side waits until its timeout. Log
        // it so the failure is visible in `*jvm-log*`.
        crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::Log {
            level: "error".to_string(),
            text: format!("invoke-reply for id {id}: failed to complete the pending future: {e}"),
        });
    }
}

/// Completes the future for ID with a Lisp-access reply. Only the worker is attached, so the
/// completion runs here.
fn handle_lisp_reply(vm: &JavaVM, id: u64, value: crate::protocol::LispOutcome) {
    let Some(future) = take(id) else {
        crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::Log {
            level: "error".to_string(),
            text: format!("lisp-reply for unknown id {id}"),
        });
        return;
    };
    let result: jni::errors::Result<()> = vm.attach_current_thread(|env| {
        let future_local = env.new_local_ref(future.as_obj())?;
        crate::jvm::invoke::complete_future_with_reply(env, &future_local, value)
    });
    if let Err(e) = result {
        crate::lisp::inbox::submit(crate::lisp::inbox::Inbound::Log {
            level: "error".to_string(),
            text: format!("lisp-reply for id {id}: failed to complete the pending future: {e}"),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    use crate::jvm::test_support::create_test_jvm;

    /// Uses a local flag: other tests may already have set `JVM_CREATE_ATTEMPTED`.
    #[test]
    fn is_first_attempt_is_true_exactly_once() {
        let flag = AtomicBool::new(false);
        assert!(is_first_attempt(&flag), "the very first call must report true");
        assert!(!is_first_attempt(&flag), "a second call must report false");
        assert!(!is_first_attempt(&flag), "every call after that must keep reporting false");
    }

    /// Builds `test/fixtures/Greeter.java` into `target/test-fixtures/greeter.jar`, so a plain
    /// `cargo test` does not depend on `test/run-ert-tests.sh`.
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

    /// `addURL` succeeds for any path, so load `Greeter` through the loader and call it.
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
    #[test]
    fn register_then_take_returns_once() {
        let vm = create_test_jvm();
        vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            let object_class = env.find_class(jni::jni_str!("java/lang/Object"))?;
            let obj = env.new_object(&object_class, jni::jni_sig!(() -> void), &[])?;
            let global = env.new_global_ref(&obj)?;
            let id = next_id();
            register(id, global);
            assert!(take(id).is_some());
            assert!(take(id).is_none());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn take_of_unknown_id_is_none() {
        assert!(take(999_999).is_none());
    }
}
