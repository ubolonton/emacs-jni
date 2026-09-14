//! Process-global handles to the running JVM, shared between the worker
//! thread that creates them (`jvm_thread::run`) and the Emacs main thread
//! that needs them for the synchronous `jvm--call-direct` path (see plan
//! `docs/superpowers/plans/2026-09-13-sync-lisp-clojure-plan.md`).
//!
//! The async `jvm--call` path never reads these: the worker keeps its own
//! `vm`/`loader` on its stack. These exist only to hand the *same* `JavaVM`
//! and shared classloader to the main thread, so a direct call resolves
//! jar-loaded classes exactly as `handle_call` does.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::ThreadId;

use jni::objects::JObject;
use jni::refs::Global;
use jni::vm::JavaVM;

/// The shared jar classloader, wrapped exactly like `jvm_thread`'s own
/// `loader` local: `Arc<Global<..>>`, because `Global` has no `Clone` impl in
/// jni 0.22, so the `Arc` is what lets the main thread cheaply take a
/// reference without a live `Env`.
pub type SharedLoader = Arc<Global<JObject<'static>>>;

static VM: Mutex<Option<JavaVM>> = Mutex::new(None);
static LOADER: Mutex<Option<SharedLoader>> = Mutex::new(None);

/// The Emacs main thread — the thread that loaded the module (`init` runs on
/// it). Recorded once so `call_direct` can permanently attach *only* this
/// thread to the JVM, giving it a stable `java.lang.Thread` peer (hence a
/// stable `Thread.currentThread()` identity and persistent thread-locals such
/// as Clojure dynamic bindings). A direct call on any other thread (a
/// `make-thread` Lisp thread) is never recorded here, so it uses a scoped
/// attach that detaches on return — transient, no leaked attachment.
static MAIN_THREAD: OnceLock<ThreadId> = OnceLock::new();

/// Whether the main thread currently holds its one-time permanent attachment.
/// Set true after `call_direct` attaches it, reset when the JVM stops (so a
/// later restart re-attaches on the next direct call).
static MAIN_ATTACHED: AtomicBool = AtomicBool::new(false);

/// Stores the `JavaVM` (Clone in jni 0.22) once the worker creates it.
pub fn set_vm(vm: JavaVM) {
    *VM.lock().unwrap() = Some(vm);
}

/// A clone of the running `JavaVM`, or `None` if no JVM is live.
pub fn vm() -> Option<JavaVM> {
    VM.lock().unwrap().clone()
}

/// Mirrors the worker's shared classloader once a jar/classpath entry is
/// loaded (`handle_load_jar`); `None` until then.
pub fn set_loader(loader: SharedLoader) {
    *LOADER.lock().unwrap() = Some(loader);
}

/// A clone of the shared classloader `Arc`, or `None` if no jar was loaded.
pub fn loader() -> Option<SharedLoader> {
    LOADER.lock().unwrap().clone()
}

/// Records the Emacs main thread. Called from module `init`, which runs on
/// it. Idempotent — a second call (e.g. a reload) keeps the first id.
pub fn set_main_thread() {
    let _ = MAIN_THREAD.set(std::thread::current().id());
}

/// Whether the caller is the Emacs main thread. False before `init` ran, and
/// on every other thread (a `make-thread` Lisp thread, or the worker).
pub fn on_main_thread() -> bool {
    MAIN_THREAD.get().is_some_and(|id| *id == std::thread::current().id())
}

/// Whether the main thread holds its permanent JVM attachment.
pub fn main_attached() -> bool {
    MAIN_ATTACHED.load(Ordering::SeqCst)
}

/// Records that the main thread now holds (true) or has dropped (false) its
/// permanent attachment.
pub fn set_main_attached(attached: bool) {
    MAIN_ATTACHED.store(attached, Ordering::SeqCst);
}

/// Clears the shared handles when the JVM stops. The main thread must not keep
/// a stale `JavaVM` after `destroy`. Also clears `MAIN_ATTACHED` so a restart
/// re-attaches the main thread on its next direct call (the main thread itself
/// is detached separately, in the `jvm--stop` defun, which runs on it).
pub fn clear() {
    *VM.lock().unwrap() = None;
    *LOADER.lock().unwrap() = None;
    MAIN_ATTACHED.store(false, Ordering::SeqCst);
}
