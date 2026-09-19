//! The worker thread's `Cmd` message envelope, its event loop and
//! lifecycle, and the correlation-id table for in-flight replies.
//!
//! Merges the former `worker.rs` (the `Sender` handle: spawning/resuming
//! the thread, submitting a `Cmd`), `pending.rs` (the table of
//! `CompletableFuture`s awaiting a Lisp reply, keyed by correlation id),
//! and the loop/lifecycle half of the former `jvm_thread.rs` (`run`'s
//! dispatch loop, JVM creation, pause/stop/resume, jar loading, and
//! completing pending replies). The reflection call machinery
//! (`resolve_and_invoke`, `invoke_method`, `invoke_constructor`,
//! `call_direct`, `handle_call`, ...) lives in `jvm::invoke`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use jni::objects::JObject;
use jni::refs::Global;
use jni::vm::{InitArgsBuilder, JavaVM};
use jni::JNIVersion;

use crate::channel;
use crate::jvm::convert::JvmObject;
use crate::jvm::runtime::STATUS;
use crate::protocol::{BridgeValue, CallTarget, LispOutcome, Status};

#[derive(Debug)]
pub(crate) enum Cmd {
    /// Sent to an already-running worker thread to resume from `paused`.
    /// A first-time start never goes through this enum at all — see
    /// `start_or_resume`, which spawns a fresh thread with
    /// classpath/options as plain arguments instead.
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
    /// Completes a pending `LispFunction.call` future (`jvm::entry::
    /// lisp_function_call`) with the Lisp callback's outcome — sent by
    /// `jvm--invoke-reply` once `jvm--resolve-invoke` (`elisp/jvm.el`) has
    /// funcalled the callback and marshalled its return value or its real
    /// `condition-case` signal into a `LispOutcome`. See
    /// `handle_invoke_reply`.
    InvokeReply {
        id: u64,
        outcome: LispOutcome,
    },
    /// A Lisp `user-ptr` wrapping a JVM object was GC'd; drop its reference
    /// on the attached worker thread (see `jvm::convert::JvmHandle::Drop`).
    ReleaseRef {
        object: JvmObject,
    },
    /// Completes a pending Lisp-access native's reply future (`intern`,
    /// `typeOf`, `isNotNil`, `eq`, `applyAsync`, `prin1ToString`) — sent by
    /// `jvm--service-inbound` (`lisp_requests::service_inbound`) after
    /// running the corresponding `Env` op on the main thread. See
    /// `handle_lisp_reply`.
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

/// Resumes the existing worker thread if one is alive (paused case), or
/// spawns a brand new one (first start, or restart after stop/crash).
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

/// Sends a non-Resume command to the live worker thread. Returns `Err(())`
/// if there is no worker thread currently running (shouldn't happen if
/// callers check `Status` first, but kept fallible rather than panicking).
pub(crate) fn submit(cmd: Cmd) -> Result<(), ()> {
    let guard = SENDER.lock().unwrap();
    match guard.as_ref() {
        Some(sender) => sender.send(cmd).map_err(|_| ()),
        None => Err(()),
    }
}

/// Called by the worker thread itself right before it exits for good
/// (successful stop, or an unrecoverable panic), so the next `jvm-start`
/// knows to spawn a fresh thread instead of trying to resume a dead one.
pub(crate) fn clear_sender() {
    *SENDER.lock().unwrap() = None;
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
// Entries here are bare `Global`s, not `Arc`-wrapped: a pending invocation's
// `CompletableFuture` global ref has exactly one owner at a time (this
// table, until `take` hands it off to `handle_invoke_reply`), never a
// "resolve while still present" access pattern that would need cheap,
// repeatable cloning -- so `Global`'s missing `Clone` impl (jni 0.22.4) is
// a non-issue here.
static TABLE: Mutex<Option<HashMap<u64, Global<JObject<'static>>>>> = Mutex::new(None);

fn table() -> std::sync::MutexGuard<'static, Option<HashMap<u64, Global<JObject<'static>>>>> {
    let mut guard = TABLE.lock().unwrap();
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    guard
}

/// Allocates a fresh correlation id for a new `invoke` round trip. Separate
/// from `register` (rather than folded into it) because
/// `jvm::entry::lisp_function_call` needs the id before it has a `Global` to
/// register -- the id goes out on the wire as part of `(invoke ID HANDLE
/// ARGS)` before the `CompletableFuture` object even exists.
pub(crate) fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::SeqCst)
}

/// Registers FUTURE (a global ref to a `java.util.concurrent.CompletableFuture`)
/// under ID, for later completion by `Cmd::InvokeReply`.
pub(crate) fn register(id: u64, future: Global<JObject<'static>>) {
    table().as_mut().unwrap().insert(id, future);
}

/// Removes and returns the pending future for ID, if any — a reply should
/// only ever resolve a given id once, so this consumes the entry rather
/// than just peeking.
fn take(id: u64) -> Option<Global<JObject<'static>>> {
    table().as_mut().unwrap().remove(&id)
}

/// Entry point for the JVM worker thread. Runs for the JVM's entire
/// lifetime: creates the JVM once, then loops handling commands until a
/// successful `Cmd::Stop` (or, from Task 8 on, an unrecoverable panic).
pub(crate) fn run(receiver: Receiver<Cmd>, classpath: Vec<String>, options: Vec<String>) {
    let vm = match create_jvm(&classpath, &options) {
        Ok(vm) => {
            // Publish the VM so the Emacs main thread can attach for a
            // synchronous `jvm--call-direct` (see `runtime` and `call_direct`).
            crate::jvm::runtime::set_vm(vm.clone());
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
                // Never handled inline here — see `handle_call`'s doc
                // comment: an arbitrary Java call can take a long time for
                // reasons unrelated to callbacks, and must not stall other
                // pending commands on this dispatch loop.
                let vm = vm.clone();
                let loader = crate::jvm::runtime::loader();
                std::thread::spawn(move || {
                    crate::jvm::invoke::handle_call(&vm, loader, id, target, member, args)
                });
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
                clear_sender();
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
// whether a `jvm::entry::define_classes` failure should be fatal to `create_jvm`
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
// see `start_or_resume`/`clear_sender`), so a single `swap` right
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
    // classes (Task 1) into this JVM, per `jvm::entry::define_classes`'s own doc
    // comment: "must be called once, right after the JVM is created,
    // before anything else in this crate references those classes." No
    // caller actually did this at runtime before Task 6 (only
    // `shim.rs`'s own unit test, against its own throwaway JVM) — Task 6
    // is the first thing that references a shim class outside that test
    // (`jvm::convert::from_bridge`'s `BridgeValue::Callback` arm calls
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
    //   either way, and (per `jvm::entry::define_classes`'s own idempotency
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
    let result: jni::errors::Result<()> =
        vm.attach_current_thread(crate::jvm::entry::define_classes);
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
    crate::jvm::runtime::clear();
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
    clear_sender();
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
        let loader = crate::jvm::runtime::get_or_create_loader(|| {
            create_shared_classloader(env).map(Arc::new)
        })?;
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
/// path through `jvm::runtime::get_or_create_loader`. Returns an error string (a
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
        let loader = crate::jvm::runtime::get_or_create_loader(|| {
            create_shared_classloader(env).map(Arc::new)
        })?;
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
/// the other half of the round trip `jvm::entry::lisp_function_call` starts: that
/// function enqueues an `Inbound::Invoke` and returns immediately; this
/// function runs once Lisp's `jvm--invoke-reply` sends back
/// `Cmd::InvokeReply` with the Lisp callback's outcome — its real return
/// value, or the real `condition-case` signal it raised, already marshalled
/// to a `LispOutcome`. Identical shape to `handle_lisp_reply` below (both
/// just hand their `LispOutcome` to `jvm::invoke::complete_future_with_reply`)
/// — the callback round trip and the Clojure apply round trip complete their
/// futures exactly the same way.
fn handle_invoke_reply(vm: &JavaVM, id: u64, outcome: crate::protocol::LispOutcome) {
    let Some(future) = take(id) else {
        crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::Log {
            level: "error".to_string(),
            text: format!("invoke-reply for unknown id {id}"),
        });
        channel::poke();
        return;
    };
    let result: jni::errors::Result<()> = vm.attach_current_thread(|env| {
        let future_local = env.new_local_ref(future.as_obj())?;
        crate::jvm::invoke::complete_future_with_reply(env, &future_local, outcome)
    });
    if let Err(e) = result {
        // The future is already removed from the pending table by this
        // point (via `take` above) and there's no other channel to report
        // this through -- the Java side just sees its future hang until
        // its own wait times out. Logging here at least makes the failure
        // visible in `*jvm-log*` rather than disappearing silently.
        crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::Log {
            level: "error".to_string(),
            text: format!("invoke-reply for id {id}: failed to complete the pending future: {e}"),
        });
        channel::poke();
    }
}

/// Resolves the pending `CompletableFuture` for ID with VALUE — the
/// Lisp-access counterpart of `handle_invoke_reply` above, completing (or
/// exceptionally completing) whatever future `jvm::entry`'s `Lisp.*` natives
/// registered in the pending table (see design spec, "Request/reply flow",
/// step 3: "Completion must route through the worker because only it is
/// attached to the JVM"). Runs inline, like `handle_invoke_reply`, for the
/// same reason: completing a future is one or two cheap JNI calls, not an
/// arbitrary user method.
fn handle_lisp_reply(vm: &JavaVM, id: u64, value: crate::protocol::LispOutcome) {
    let Some(future) = take(id) else {
        crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::Log {
            level: "error".to_string(),
            text: format!("lisp-reply for unknown id {id}"),
        });
        channel::poke();
        return;
    };
    let result: jni::errors::Result<()> = vm.attach_current_thread(|env| {
        let future_local = env.new_local_ref(future.as_obj())?;
        crate::jvm::invoke::complete_future_with_reply(env, &future_local, value)
    });
    if let Err(e) = result {
        crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::Log {
            level: "error".to_string(),
            text: format!("lisp-reply for id {id}: failed to complete the pending future: {e}"),
        });
        channel::poke();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    /// Covers the exact swap semantics `create_jvm` relies on to decide
    /// whether a `jvm::entry::define_classes` failure is fatal (first attempt)
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

    /// Retries against jni-rs 0.22.4's racy check-then-create singleton --
    /// see `src/jvm/convert.rs`'s `create_test_jvm` for the full
    /// explanation. This test used to create its JVM directly; with more
    /// JVM-creating tests now running in this same binary (`src/jvm/entry.rs`), the
    /// race became reliably hit rather than a rare fluke.
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
