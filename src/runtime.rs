//! Process-global handles to the running JVM, shared between the worker
//! thread that creates them (`jvm_thread::run`) and the Emacs main thread
//! that needs them for the synchronous `jvm--call-direct` path (see plan
//! `docs/superpowers/plans/2026-09-13-sync-lisp-clojure-plan.md`).
//!
//! The async `jvm--call` path never reads these: the worker keeps its own
//! `vm`/`loader` on its stack. These exist only to hand the *same* `JavaVM`
//! and shared classloader to the main thread, so a direct call resolves
//! jar-loaded classes exactly as `handle_call` does.

use std::sync::{Arc, Mutex};

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

/// Clears both handles when the JVM stops. The main thread must not keep a
/// stale `JavaVM` after `destroy`.
pub fn clear() {
    *VM.lock().unwrap() = None;
    *LOADER.lock().unwrap() = None;
}
