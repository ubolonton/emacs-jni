use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use jni::vm::{InitArgsBuilder, JavaVM};
use jni::JNIVersion;

use crate::commands::Cmd;
use crate::marshal::marshal_error;
use crate::protocol::Status;
use crate::status::STATUS;
use crate::{channel, worker};

/// Entry point for the JVM worker thread. Runs for the JVM's entire
/// lifetime: creates the JVM once, then loops handling commands until a
/// successful `Cmd::Stop` (or, from Task 8 on, an unrecoverable panic).
pub fn run(receiver: Receiver<Cmd>, classpath: Vec<String>, options: Vec<String>) {
    let vm = match create_jvm(&classpath, &options) {
        Ok(vm) => {
            // Publish the VM so the Emacs main thread can attach for a
            // synchronous `jvm--call-direct` (see `runtime` and `call_direct`).
            crate::runtime::set_vm(vm.clone());
            STATUS.finish(Status::Ready);
            crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::Status {
                status: Status::Ready,
                detail: None,
            });
            channel::poke();
            vm
        }
        Err(e) => {
            STATUS.finish(Status::StartFailed);
            crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::Status {
                status: Status::StartFailed,
                detail: Some(e),
            });
            channel::poke();
            worker::clear_sender();
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
                // Never handled inline here — see `handle_call`'s doc
                // comment: an arbitrary Java call can take a long time for
                // reasons unrelated to callbacks, and must not stall other
                // pending commands on this dispatch loop.
                let vm = vm.clone();
                let loader = crate::runtime::loader();
                std::thread::spawn(move || handle_call(&vm, loader, id, target, member, args));
                false
            }
            Cmd::InvokeReply { id, outcome } => {
                // Handled inline, unlike `Cmd::Call` — see `handle_invoke_reply`'s
                // own doc comment for why completing an already-existing
                // `CompletableFuture` doesn't need its own thread.
                handle_invoke_reply(&vm, id, outcome);
                false
            }
            // Handled inline, like `Cmd::InvokeReply` — dropping a
            // `GlobalRef` is one cheap JNI call (`DeleteGlobalRef`), not an
            // arbitrary user method (spec "Worker thread role"). Lisp's GC
            // collected a `user-ptr` wrapping a JVM object; its `Drop`
            // handed the shared reference here (its own `Drop` may run on a
            // thread with no JNI attachment). Dropping this last `Arc` runs
            // `Global::Drop`'s `DeleteGlobalRef` on this attached worker
            // thread.
            Cmd::ReleaseRef { object } => {
                std::mem::drop(object);
                false
            }
            // Handled inline, same reasoning as `Cmd::InvokeReply`: completing
            // an already-existing `CompletableFuture` is one or two cheap JNI
            // calls, not an arbitrary user method.
            Cmd::LispReply { id, value } => {
                handle_lisp_reply(&vm, id, value);
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
                crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::Status {
                    status: Status::Crashed,
                    detail: Some(message),
                });
                channel::poke();
                worker::clear_sender();
                return;
            }
        }
    }
}

// Restart-after-crash/-failure works even though `run` drops its `vm` above
// and no handle is retained anywhere in this codebase: jni-rs 0.22.4 caches
// the raw JavaVM pointer in a process-global `OnceLock` internally
// (`JAVA_VM_SINGLETON` / `impl_with_libjvm` in `jni-0.22.4/src/vm/java_vm.rs`),
// so a later `JavaVM::new()` call here transparently re-attaches to the same
// underlying VM instead of re-bootstrapping it. There's no real "attach to
// the retained `JavaVM` handle" hand-off happening, contrary to the design
// spec's more literal wording — this comment is that clarification.
// Tracks whether `create_jvm` has ever *reached the shim-loading step*
// before in this process (regardless of whether shim loading itself then
// succeeded or failed) — swapped right before that step, only once a live
// `JavaVM` actually exists (see the swap's call site below), so the very
// first call to reach that point sees `false` (and thus knows it's "the
// first attempt") and every call after that — a restart following
// `jvm-stop`/a crash, or a retry of `jvm-start` following an *earlier*,
// shim-unrelated `create_jvm` failure — sees `true`. Used below to decide
// whether a `shim::define_classes` failure should be fatal to `create_jvm`
// or merely logged; see that call site's doc comment for why the
// distinction matters.
//
// Deliberately swapped just before shim loading, not at `create_jvm`'s
// entry: an earlier `create_jvm` call can fail before ever reaching a live
// JVM (bad init args, `JavaVM::new` itself failing) for reasons that have
// nothing to do with the shim, and `jvm-start` is an explicitly supported
// retry target from `start-failed` (`status.rs`'s `begin_start` legal-from
// list includes `Status::StartFailed`). If the flag were consumed at
// `create_jvm`'s entry, that kind of failed-then-retried call would burn
// the "first attempt" slot on a call that never even tried to load the
// shim, so the *real* first successful boot (the retry) would be
// misclassified as "subsequent" and a genuinely broken shim on it would
// be silently swallowed instead of surfacing as `start-failed` --
// reproducing the exact bug this flag exists to fix, just one retry later.
// Swapping here instead means only calls that produce a live JVM (and
// therefore actually attempt shim loading) ever consume the slot,
// regardless of how many earlier calls failed for unrelated reasons.
//
// Worker threads never run `create_jvm` concurrently with each other (a
// new one is only spawned after the previous one has fully torn down —
// see `worker::start_or_resume`/`clear_sender`), so a single `swap` right
// before shim loading, with no separate check-then-set race window, is
// sufficient.
static JVM_CREATE_ATTEMPTED: AtomicBool = AtomicBool::new(false);

/// Returns whether this call is the first ever call against FLAG — `true`
/// exactly once per flag, `false` on every call after that. Factored out
/// of `create_jvm` as its own function (taking the flag as a parameter,
/// rather than reading `JVM_CREATE_ATTEMPTED` directly) purely so the swap
/// semantics it relies on are unit-testable in isolation, without needing
/// a real `JavaVM` — see the test below. `create_jvm` itself always calls
/// this with `&JVM_CREATE_ATTEMPTED`.
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

    // Defines the `emacs_jni.core.{LispCallable,LispFunction,LispNonLocalExit}` shim
    // classes (Task 1) into this JVM, per `shim::define_classes`'s own doc
    // comment: "must be called once, right after the JVM is created,
    // before anything else in this crate references those classes." No
    // caller actually did this at runtime before Task 6 (only
    // `shim.rs`'s own unit test, against its own throwaway JVM) — Task 6
    // is the first thing that references a shim class outside that test
    // (`marshal::lisp_arg_to_jvalue`'s `BridgeValue::Callback` arm calls
    // `find_class(emacs_jni/core/LispFunction)` on every `jvm-call` argument
    // that turns out to be a Lisp function), so without this, that
    // `find_class` would throw `ClassNotFoundException` against a real
    // JVM started via `jvm-start`.
    //
    // Fatal on the first attempt to reach this point (a live JVM) in this
    // process, but log-and-continue on any attempt after that — see
    // `JVM_CREATE_ATTEMPTED`'s doc comment for how "first" is determined
    // (deliberately *not* "the first call to `create_jvm`", which is a
    // subtly different, wrong condition — see that comment).
    // The two cases need different handling because they mean different
    // things:
    //
    // - First attempt: this is a genuinely fresh JVM. `define_classes`
    //   failing here means the shim itself is broken (missing/corrupt
    //   `.class` bytes, a `javac`/JDK mismatch, etc.) — a real problem
    //   that should surface immediately and clearly as `jvm-start`
    //   reporting `start-failed`, not be swallowed only to resurface
    //   later as a confusing `ClassNotFoundException`/marshal error deep
    //   inside some unrelated `jvm-call` that happens to pass a Lisp
    //   function argument, with no obvious link back to "the shim never
    //   loaded".
    // - Subsequent attempt: a *restart* after a real `jvm-stop`
    //   (`JNI_DestroyJavaVM`) or a crash. The underlying VM can come back
    //   as a non-functional "zombie" that accepts `JavaVM::new`/attach
    //   calls but fails any subsequent real JNI work — a pre-existing,
    //   already-documented risk (`jvm-stop`'s own doc comment: "relies on
    //   JNI_DestroyJavaVM, which some JVM builds handle unreliably ...
    //   jvm-start afterward is best-effort"), confirmed independently of
    //   this change (a plain `jvm-load-jar`/`jvm-call` on such a VM
    //   already fails the same way, with the same "JNI call failed"
    //   message). Propagating a `define_classes` failure here through
    //   `create_jvm` would make `jvm-start` itself report `start-failed`
    //   in that scenario, where before this task it reported `ready`
    //   (with only the *next* real operation failing instead) — a
    //   behavior change with no upside, since the VM is equally unusable
    //   either way, and (per `shim::define_classes`'s own idempotency
    //   doc comment) the shim classes are typically already loaded from
    //   the first successful boot regardless. Logging and returning the
    //   VM as usual keeps `jvm-start`'s success/failure semantics exactly
    //   what they were before this task in the zombie-VM case.
    //
    // The flag is swapped here, not at `create_jvm`'s entry, specifically
    // so an earlier call that failed before ever reaching a live JVM (bad
    // init args, etc.) doesn't consume the "first attempt" slot — see
    // `JVM_CREATE_ATTEMPTED`'s doc comment.
    let is_first_attempt = is_first_attempt(&JVM_CREATE_ATTEMPTED);
    let result: jni::errors::Result<()> = vm.attach_current_thread(crate::shim::define_classes);
    match result {
        Ok(()) => {}
        Err(e) if is_first_attempt => return Err(e.to_string()),
        Err(e) => {
            crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::Log {
                level: "error".to_string(),
                text: format!("failed to define emacs_jni.core shim classes: {e}"),
            });
            channel::poke();
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
            crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::Status {
                status: Status::Paused,
                detail: None,
            });
            channel::poke();
        }
        Err(e) => {
            // Detach failing is unexpected; stay in `pausing` rather than
            // claiming success we can't back up, and surface it as a log
            // line rather than a status (there's no `pause-failed` state
            // in the spec's state machine, so pausing simply never
            // completes — the caller's `-sync` wrapper times out).
            crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::Log {
                level: "error".to_string(),
                text: format!("jvm-pause: detach_current_thread failed: {e}"),
            });
            channel::poke();
        }
    }
}

fn handle_stop(vm: JavaVM) {
    // Safety: `destroy` requires no Java methods on the calling thread's
    // stack (true here — we're between command handlers) and that the
    // JavaVM isn't used again afterward — nothing below touches `vm`.
    let result = unsafe { vm.destroy() };
    // Drop the shared handles the main thread reads for `call_direct`: the VM
    // is gone now. The main thread already detached itself in the `jvm--stop`
    // defun (before this `Cmd::Stop` was sent), so no main-thread attachment
    // was left to block this `destroy`. `clear` also resets the
    // main-attached flag so a restart re-attaches on the next direct call.
    crate::runtime::clear();
    match result {
        Ok(()) => {
            STATUS.finish(Status::Stopped);
            crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::Status {
                status: Status::Stopped,
                detail: None,
            });
            channel::poke();
        }
        Err(e) => {
            STATUS.finish(Status::StopFailed);
            crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::Status {
                status: Status::StopFailed,
                detail: Some(e.to_string()),
            });
            channel::poke();
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
            crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::Status {
                status: Status::Ready,
                detail: None,
            });
            channel::poke();
        }
        Err(e) => {
            STATUS.finish(Status::StartFailed);
            crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::Status {
                status: Status::StartFailed,
                detail: Some(e.to_string()),
            });
            channel::poke();
        }
    }
}

fn handle_load_jar(vm: &JavaVM, id: u64, path: String) {
    // `URLClassLoader#addURL` is lazy: it just appends to the loader's
    // internal URL list and never inspects the target until a class is
    // actually looked up, so it does not itself fail for a nonexistent
    // path (verified empirically -- see Task 7's report). Check existence
    // up front so `jvm-load-jar`'s ok/err result actually reflects whether
    // the jar is there, without touching the reflective addURL sequence
    // below.
    //
    // Accepts a directory too, not just a jar file: `add_jar_url` below
    // goes through `File::toURI`, which already appends a trailing slash
    // for a directory on its own -- exactly the URL form `URLClassLoader`
    // needs to treat it as a classpath directory (rather than a jar) --
    // so this primitive doubles as "add a classpath directory", used by
    // `jvm-clj-start` to add `clojure-src/` alongside real jars.
    if !std::path::Path::new(&path).exists() {
        crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::LoadJarDone {
            id,
            ok: false,
            err: Some(format!("jar/classpath entry not found: {path}")),
        });
        channel::poke();
        return;
    }

    let result: jni::errors::Result<()> = vm.attach_current_thread(|env| {
        let loader =
            crate::runtime::get_or_create_loader(|| create_shared_classloader(env).map(Arc::new))?;
        let loader_local = env.new_local_ref(loader.as_obj())?;
        add_jar_url(env, &loader_local, &path)
    });

    match result {
        Ok(()) => crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::LoadJarDone {
            id,
            ok: true,
            err: None,
        }),
        Err(e) => crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::LoadJarDone {
            id,
            ok: false,
            err: Some(e.to_string()),
        }),
    }
    channel::poke();
}

/// Synchronous main-thread counterpart of `handle_load_jar`: adds PATH to the
/// shared classloader on *this* thread, sharing the one loader with the async
/// path through `runtime::get_or_create_loader`. Returns an error string (a
/// missing path, or a JNI failure) instead of enqueueing an
/// `Inbound::LoadJarDone`, since the caller (`jvm--load-jar-direct`) returns
/// or signals directly. See `call_direct` for the main-thread attach model.
pub(crate) fn load_jar_direct(vm: &JavaVM, path: &str) -> Result<(), String> {
    // Same up-front existence check as `handle_load_jar`: `addURL` is lazy and
    // never fails for a nonexistent path on its own.
    if !std::path::Path::new(path).exists() {
        return Err(format!("jar/classpath entry not found: {path}"));
    }
    // A *scoped* attach: a no-op on the permanently-attached main thread, an
    // owning attach-and-detach on any other caller. Unlike `call_direct`, this
    // never takes the permanent attach -- `addURL` needs no stable thread
    // identity, so there is no reason to touch the main-attachment state.
    let result: jni::errors::Result<()> = vm.attach_current_thread_for_scope(|env| {
        let loader =
            crate::runtime::get_or_create_loader(|| create_shared_classloader(env).map(Arc::new))?;
        let loader_local = env.new_local_ref(loader.as_obj())?;
        add_jar_url(env, &loader_local, path)
    });
    result.map_err(|e| e.to_string())
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
        &[jni::objects::JValue::Object(loader), jni::objects::JValue::Object(&invoke_args)],
    )?;

    Ok(())
}

/// Resolves the pending `CompletableFuture` for ID — inline, cheaply, on
/// the worker thread's own persistent JNI attachment, per spec "Worker
/// thread role": unlike `Cmd::Call`, this never spawns a thread, since
/// completing a future is not itself a slow operation (it's a single JNI
/// call to `CompletableFuture#complete`/`#completeExceptionally`, not an
/// arbitrary user method that might block for arbitrary reasons). This is
/// the other half of the round trip `shim::lisp_function_call` starts: that
/// function enqueues an `Inbound::Invoke` and returns immediately; this
/// function runs once Lisp's `jvm--invoke-reply` sends back
/// `Cmd::InvokeReply` with the Lisp callback's outcome — its real return
/// value, or the real `condition-case` signal it raised, already marshalled
/// to a `LispOutcome`. Identical shape to `handle_lisp_reply` below (both
/// just hand their `LispOutcome` to `complete_future_with_reply`) — the
/// callback round trip and the Clojure apply round trip complete their
/// futures exactly the same way.
fn handle_invoke_reply(vm: &JavaVM, id: u64, outcome: crate::protocol::LispOutcome) {
    let Some(future) = crate::pending::take(id) else {
        crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::Log {
            level: "error".to_string(),
            text: format!("invoke-reply for unknown id {id}"),
        });
        channel::poke();
        return;
    };
    let result: jni::errors::Result<()> = vm.attach_current_thread(|env| {
        let future_local = env.new_local_ref(future.as_obj())?;
        complete_future_with_reply(env, &future_local, outcome)
    });
    if let Err(e) = result {
        // The future is already removed from `pending` by this point (via
        // `take` above) and there's no other channel to report this
        // through -- the Java side just sees its future hang until its own
        // wait times out. Logging here at least makes the failure visible
        // in `*jvm-log*` rather than disappearing silently.
        crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::Log {
            level: "error".to_string(),
            text: format!("invoke-reply for id {id}: failed to complete the pending future: {e}"),
        });
        channel::poke();
    }
}

/// Resolves the pending `CompletableFuture` for ID with VALUE — the
/// Lisp-access counterpart of `handle_invoke_reply` above, completing (or
/// exceptionally completing) whatever future `shim.rs`'s `Lisp.*` natives
/// registered in `pending` (see design spec, "Request/reply flow", step
/// 3: "Completion must route through the worker because only it is
/// attached to the JVM"). Runs inline, like `handle_invoke_reply`, for the
/// same reason: completing a future is one or two cheap JNI calls, not an
/// arbitrary user method.
fn handle_lisp_reply(vm: &JavaVM, id: u64, value: crate::protocol::LispOutcome) {
    let Some(future) = crate::pending::take(id) else {
        crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::Log {
            level: "error".to_string(),
            text: format!("lisp-reply for unknown id {id}"),
        });
        channel::poke();
        return;
    };
    let result: jni::errors::Result<()> = vm.attach_current_thread(|env| {
        let future_local = env.new_local_ref(future.as_obj())?;
        complete_future_with_reply(env, &future_local, value)
    });
    if let Err(e) = result {
        crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::Log {
            level: "error".to_string(),
            text: format!("lisp-reply for id {id}: failed to complete the pending future: {e}"),
        });
        channel::poke();
    }
}

/// Builds the actual Java value a successful `LispOutcome` becomes —
/// factored out of `handle_lisp_reply` since the `LispOutcome::Signal`/
/// `Throw` cases (handled by that function's caller) never reach here.
///
/// `pub(crate)` because the reentrant direct path (`shim.rs`) builds the same
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
            let jvalue = crate::marshal::lisp_arg_to_jvalue(env, &bridge, &object_class)?;
            crate::marshal::box_if_primitive(env, jvalue)
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
/// future) and the reentrant direct path (`shim.rs`'s `applyAsync` fast path,
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
/// direct-path native (`shim.rs`'s synchronous natives), so it propagates up
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
/// on its own short-lived thread (spawned by `run`'s `Cmd::Call` arm above) —
/// never inline on the worker's dispatch loop, per spec "Worker thread
/// role": an arbitrary Java call can take a long time for reasons unrelated
/// to callbacks, and must not stall other pending commands.
fn handle_call(
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
    loader: &Option<crate::runtime::SharedLoader>,
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
    loader: &Option<crate::runtime::SharedLoader>,
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
    if crate::runtime::on_main_thread()
        && !crate::runtime::main_attached()
        && vm.attach_current_thread(|_env| Ok::<(), jni::errors::Error>(())).is_ok()
    {
        crate::runtime::set_main_attached(true);
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
/// `shim::object_to_lisp_request_arg` classifier the inbound direction built
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
            let a = crate::shim::object_to_lisp_request_arg(env, a_obj)?;
            let b = crate::shim::object_to_lisp_request_arg(env, b_obj)?;
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
        return crate::marshal::jvm_value_to_lisp_arg(
            env,
            jni::objects::JValueOwned::Object(result),
        );
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
        return crate::marshal::jvm_value_to_lisp_arg(
            env,
            jni::objects::JValueOwned::Object(instance),
        );
    }

    Err(last_arg_mismatch.map_or_else(
        || marshal_error(&format!("no constructor with {} argument(s) found", args.len())),
        |caught| describe_caught_java_exception(env, caught),
    ))
}

/// Shared by `invoke_method`/`invoke_constructor`: marshals ARGS against
/// PARAM_TYPES's declared types (in that order — `lisp_arg_to_jvalue` is
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
        let jvalue = crate::marshal::lisp_arg_to_jvalue(env, arg, &param_type)?;
        let boxed = crate::marshal::box_if_primitive(env, jvalue)?;
        invoke_args.set_element(env, i, boxed)?;
    }
    Ok(invoke_args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    /// Covers the exact swap semantics `create_jvm` relies on to decide
    /// whether a `shim::define_classes` failure is fatal (first attempt)
    /// or merely logged (a restart) — see `JVM_CREATE_ATTEMPTED`'s and
    /// `is_first_attempt`'s doc comments. Uses its own local flag rather
    /// than the module's shared `JVM_CREATE_ATTEMPTED` static, since that
    /// static is process-global and other tests in this binary (run with
    /// `--test-threads=1`, so sharing one process) may have already
    /// flipped it by the time this test runs -- this test is about the
    /// swap mechanism itself, not about `JVM_CREATE_ATTEMPTED`'s
    /// particular state at some point in the suite.
    #[test]
    fn is_first_attempt_is_true_exactly_once() {
        let flag = AtomicBool::new(false);
        assert!(is_first_attempt(&flag), "the very first call must report true");
        assert!(!is_first_attempt(&flag), "a second call must report false");
        assert!(!is_first_attempt(&flag), "every call after that must keep reporting false");
    }

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
