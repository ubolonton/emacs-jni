use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use jni::vm::{InitArgsBuilder, JavaVM};
use jni::JNIVersion;

use crate::commands::Cmd;
use crate::marshal::marshal_error;
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
    // loaded jar shares one classloader (see spec: "Jar loading"). `Arc`-
    // wrapped (not a bare `Global`, which has no `Clone` impl in jni 0.22 —
    // same reasoning as `handles::TABLE`) so `Cmd::Call`'s per-call thread
    // (Task 5) can cheaply clone out a reference to it without needing a
    // live `Env` on this thread.
    let mut loader: Option<Arc<jni::refs::Global<jni::objects::JObject<'static>>>> = None;

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
            Cmd::Call { id, target, member, args } => {
                // Never handled inline here — see `handle_call`'s doc
                // comment: an arbitrary Java call can take a long time for
                // reasons unrelated to callbacks, and must not stall other
                // pending commands on this dispatch loop.
                let vm = vm.clone();
                let loader = loader.clone();
                std::thread::spawn(move || handle_call(&vm, loader, id, target, member, args));
                false
            }
            Cmd::InvokeReply { id, ok, result } => {
                // Handled inline, unlike `Cmd::Call` — see `handle_invoke_reply`'s
                // own doc comment for why completing an already-existing
                // `CompletableFuture` doesn't need its own thread.
                handle_invoke_reply(&vm, id, ok, result);
                false
            }
            // Handled inline, like `Cmd::InvokeReply` — dropping a
            // `GlobalRef` is a single cheap JNI call (`DeleteGlobalRef`),
            // not an arbitrary user method, so it doesn't need its own
            // thread (spec "Worker thread role"). This is the other half
            // of `handles::JvmHandle::Drop`'s round trip: Lisp's GC
            // collected a `user-ptr` wrapping a JVM object, which posted
            // this command instead of releasing the ref directly (that
            // `Drop` may run on a thread with no JNI attachment at all —
            // see its own doc comment), and now the worker thread (which
            // *is* attached) actually drops it.
            Cmd::ReleaseRef { handle } => {
                crate::handles::release(handle);
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

    // Defines the `emacs_jni.{LispCallable,LispFunction,LispError}` shim
    // classes (Task 1) into this JVM, per `shim::define_classes`'s own doc
    // comment: "must be called once, right after the JVM is created,
    // before anything else in this crate references those classes." No
    // caller actually did this at runtime before Task 6 (only
    // `shim.rs`'s own unit test, against its own throwaway JVM) — Task 6
    // is the first thing that references a shim class outside that test
    // (`marshal::lisp_arg_to_jvalue`'s `LispArg::Callback` arm calls
    // `find_class(emacs_jni/LispFunction)` on every `jvm-call` argument
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
            channel::send(&protocol::log_message(
                "error",
                &format!("failed to define emacs_jni shim classes: {e}"),
            ));
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
    loader: &mut Option<Arc<jni::refs::Global<jni::objects::JObject<'static>>>>,
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
            *loader = Some(Arc::new(create_shared_classloader(env)?));
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

/// Resolves the pending `CompletableFuture` for ID — inline, cheaply, on
/// the worker thread's own persistent JNI attachment, per spec "Worker
/// thread role": unlike `Cmd::Call`, this never spawns a thread, since
/// completing a future is not itself a slow operation (it's a single JNI
/// call to `CompletableFuture#complete`/`#completeExceptionally`, not an
/// arbitrary user method that might block for arbitrary reasons). This is
/// the other half of the round trip `shim::lisp_function_call` starts:
/// that function posts `(invoke ID HANDLE ARGS)` and returns immediately;
/// this function runs once Lisp's `jvm--invoke-reply` sends back
/// `Cmd::InvokeReply` with the Lisp function's result (or error).
fn handle_invoke_reply(vm: &JavaVM, id: u64, ok: bool, result: crate::commands::LispArg) {
    let Some(future) = crate::pending::take(id) else {
        channel::send(&protocol::log_message("error", &format!("invoke-reply for unknown id {id}")));
        return;
    };
    let result: jni::errors::Result<()> = vm.attach_current_thread(|env| {
        let future_local = env.new_local_ref(future.as_obj())?;
        if ok {
            // `LispCallable#call` is declared to return `CompletableFuture<Object>`
            // -- `Object` is the only param type `lisp_arg_to_jvalue` ever
            // needs to marshal against here, matching how `Method.invoke`'s
            // own `Object[]` args array (Task 5's `build_invoke_args`) does
            // the same "unknown declared type, box everything" thing.
            let param_type = env.find_class(jni::jni_str!("java/lang/Object"))?;
            let jvalue = crate::marshal::lisp_arg_to_jvalue(env, &result, &param_type)?;
            let boxed = crate::marshal::box_if_primitive(env, jvalue)?;
            env.call_method(
                &future_local,
                jni::jni_str!("complete"),
                jni::jni_sig!((v: java.lang.Object) -> boolean),
                &[jni::objects::JValueOwned::Object(boxed).borrow()],
            )?;
        } else {
            // The Lisp side (`jvm--invoke-reply`) always sends a formatted
            // error message as a plain string for a failed callback (see
            // `jvm_invoke_reply` in `lib.rs`) -- any other shape here would
            // mean a protocol-level bug, not a normal application error, so
            // there's nothing sensible to complete the future with; just
            // leave it pending rather than fabricate a result.
            let crate::commands::LispArg::Str(message) = result else {
                return Ok(());
            };
            let lisp_error_class = env.find_class(jni::jni_str!("emacs_jni/LispError"))?;
            let jmessage = env.new_string(&message)?;
            let error = env.new_object(
                &lisp_error_class,
                jni::jni_sig!((message: java.lang.String) -> void),
                &[jni::objects::JValueOwned::Object(jmessage.into()).borrow()],
            )?;
            env.call_method(
                &future_local,
                jni::jni_str!("completeExceptionally"),
                jni::jni_sig!((ex: java.lang.Throwable) -> boolean),
                &[jni::objects::JValueOwned::Object(error).borrow()],
            )?;
        }
        Ok(())
    });
    if let Err(e) = result {
        // The future is already removed from `pending` by this point (via
        // `take` above) and there's no other channel to report this
        // through -- the Java side just sees its future hang until its own
        // wait times out. Logging here at least makes the failure visible
        // in `*jvm-log*` rather than disappearing silently.
        channel::send(&protocol::log_message(
            "error",
            &format!("invoke-reply for id {id}: failed to complete the pending future: {e}"),
        ));
    }
}

/// Handles a `Cmd::Call`: resolves TARGET.MEMBER reflectively and invokes
/// it with ARGS, then sends a `call-done` message with the result. Runs on
/// its own short-lived thread (spawned by `run`'s `Cmd::Call` arm above) —
/// never inline on the worker's dispatch loop, per spec "Worker thread
/// role": an arbitrary Java call can take a long time for reasons unrelated
/// to callbacks, and must not stall other pending commands.
fn handle_call(
    vm: &JavaVM,
    loader: Option<Arc<jni::refs::Global<jni::objects::JObject<'static>>>>,
    id: u64,
    target: crate::commands::CallTarget,
    member: String,
    args: Vec<crate::commands::LispArg>,
) {
    let result: jni::errors::Result<crate::commands::LispArg> = vm.attach_current_thread(|env| {
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
        if let Some(global) = &loader {
            let loader_local = env.new_local_ref(global.as_obj())?;
            let thread_class = env.find_class(jni::jni_str!("java/lang/Thread"))?;
            let current_thread = env
                .call_static_method(&thread_class, jni::jni_str!("currentThread"), jni::jni_sig!(() -> java.lang.Thread), &[])?
                .l()?;
            env.call_method(
                &current_thread,
                jni::jni_str!("setContextClassLoader"),
                jni::jni_sig!((cl: java.lang.ClassLoader) -> void),
                &[jni::objects::JValue::Object(&loader_local)],
            )?;
        }

        let (class, instance): (jni::objects::JClass, Option<jni::objects::JObject>) = match &target {
            crate::commands::CallTarget::ClassName(name) => {
                let class = match &loader {
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
            crate::commands::CallTarget::Object(handle) => {
                let global = crate::handles::resolve(*handle)
                    .ok_or_else(|| marshal_error(&format!("unknown JVM object handle {handle}")))?;
                let local = env.new_local_ref(global.as_obj())?;
                (env.get_object_class(&local)?, Some(local))
            }
        };

        if member == "<init>" {
            invoke_constructor(env, &class, &args)
        } else {
            invoke_method(env, &class, instance.as_ref(), &member, &args)
        }
    });

    match result {
        Ok(value) => {
            let sexp = crate::marshal::lisp_arg_to_sexp(&value);
            channel::send(&protocol::call_done_message(id, true, Some(&sexp), None));
        }
        Err(e) => channel::send(&protocol::call_done_message(id, false, None, Some(&e.to_string()))),
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

/// Renders a pending Java exception (after a reflective `Method.invoke`/
/// `Constructor.newInstance` call reports `Error::JavaException`) as a
/// readable message that includes the whole `getCause()` chain, not just
/// the outermost exception.
///
/// This matters specifically for `Method.invoke`/`Constructor.newInstance`:
/// both always wrap whatever the *target* code actually threw in an
/// `InvocationTargetException`, whose own `getMessage()` is normally
/// `null` and whose own stack trace is just the two reflection frames
/// where it was constructed — the actually useful information (e.g. a
/// `LispError`'s message, for Task 7's callback round trip: `Calc.
/// callWithArg`'s `future.get(...)` throws `ExecutionException`, itself
/// wrapping the `LispError` `handle_invoke_reply` built) is one or more
/// `getCause()` hops further down. Left to `JavaVM::attach_current_thread`'s
/// own default exception handling (which auto-converts a still-pending
/// exception into `Error::CaughtJavaException` using exactly that shallow
/// info, once the whole `attach_current_thread` closure returns without
/// having cleared it), the caller only ever sees something like `"<NULL>
/// \nStack trace:\n0: ...DirectMethodHandleAccessor.invoke...\n1: ...
/// Method.invoke...\n"` — true, but useless for diagnosing what actually
/// went wrong (found empirically running this task's own ERT test: see
/// task-7-report.md). Catching and describing the exception here instead,
/// immediately after the failing reflective call and before it can escape
/// this function, lets the message include the real cause.
fn describe_pending_java_exception(env: &mut jni::Env) -> jni::errors::Error {
    let caught = match env.exception_catch() {
        Err(e) => e,
        Ok(()) => {
            return marshal_error("expected a pending Java exception (Error::JavaException) but found none")
        }
    };
    let jni::errors::Error::CaughtJavaException { name, msg, exception, .. } = caught else {
        // `exception_catch` itself failed in some unexpected way (e.g.
        // couldn't query the throwable's class/message) -- surface that
        // failure as-is rather than losing it.
        return caught;
    };

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
    args: &[crate::commands::LispArg],
) -> jni::errors::Result<crate::commands::LispArg> {
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

    for i in 0..count {
        let candidate = methods.get_element(env, i)?;
        let name = env
            .call_method(&candidate, jni::jni_str!("getName"), jni::jni_sig!(() -> java.lang.String), &[])?
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
            &[
                jni::objects::JValue::Object(receiver),
                jni::objects::JValue::Object(&invoke_args),
            ],
        ) {
            Ok(v) => v.l()?,
            // See `describe_pending_java_exception`'s doc comment: without
            // this, the caller only ever learns "a Java exception was
            // thrown", not what it actually said.
            Err(jni::errors::Error::JavaException) => return Err(describe_pending_java_exception(env)),
            Err(e) => return Err(e),
        };
        return crate::marshal::jvm_value_to_lisp_arg(env, jni::objects::JValueOwned::Object(result));
    }

    Err(marshal_error(&format!(
        "no method {method_name} with {} argument(s) found",
        args.len()
    )))
}

fn invoke_constructor(
    env: &mut jni::Env<'_>,
    class: &jni::objects::JClass,
    args: &[crate::commands::LispArg],
) -> jni::errors::Result<crate::commands::LispArg> {
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
            Err(jni::errors::Error::JavaException) => return Err(describe_pending_java_exception(env)),
            Err(e) => return Err(e),
        };
        return crate::marshal::jvm_value_to_lisp_arg(env, jni::objects::JValueOwned::Object(instance));
    }

    Err(marshal_error(&format!(
        "no constructor with {} argument(s) found",
        args.len()
    )))
}

/// Shared by `invoke_method`/`invoke_constructor`: marshals ARGS against
/// PARAM_TYPES's declared types (in that order — `lisp_arg_to_jvalue` is
/// type-directed) and boxes any primitive result, since `Method.invoke`/
/// `Constructor.newInstance` both take a plain `Object[]`.
fn build_invoke_args<'local>(
    env: &mut jni::Env<'local>,
    param_types: &jni::objects::JObjectArray<'local>,
    args: &[crate::commands::LispArg],
) -> jni::errors::Result<jni::objects::JObjectArray<'local>> {
    let object_class = env.find_class(jni::jni_str!("java/lang/Object"))?;
    let invoke_args = env.new_object_array(args.len() as i32, &object_class, jni::objects::JObject::null())?;
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
