use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use jni::objects::JObject;
use jni::refs::Global;

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);
// TODO: This table is likely removable. It existed so a JVM object could be a
// text-serialisable integer over the old sexp pipe; that pipe is gone (typed
// inbound queue). `Global` is `Send + Sync`, so `BridgeValue::Object` could carry
// `Arc<Global>` directly and the `JvmHandle` user-ptr could box it, deleting this
// map. See Item 6 in
// `docs/superpowers/specs/2026-09-18-marshalling-consolidation-design.md`.
// Entries are `Arc`-wrapped, not bare `Global`s: jni 0.22's `Global` has no
// `Clone` impl (a real clone would mean a fresh `NewGlobalRef` JNI call,
// which needs a live `Env` that `resolve` below doesn't have) -- see
// `resolve`'s doc comment.
static TABLE: Mutex<Option<HashMap<u64, Arc<Global<JObject<'static>>>>>> = Mutex::new(None);

fn table() -> std::sync::MutexGuard<'static, Option<HashMap<u64, Arc<Global<JObject<'static>>>>>> {
    let mut guard = TABLE.lock().unwrap();
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    guard
}

/// Registers a JVM object's global reference under a fresh handle,
/// returning that handle. The caller (Task 4) is responsible for actually
/// creating the `Global` ref via `env.new_global_ref` before calling this
/// — this function only owns the handle-to-ref mapping, not JNI concerns.
pub fn register(obj: Global<JObject<'static>>) -> u64 {
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::SeqCst);
    table().as_mut().unwrap().insert(handle, Arc::new(obj));
    handle
}

/// Looks up the global ref for a handle, without removing it — callers
/// needing a fresh local ref to pass into a JNI call should call
/// `env.new_local_ref` on the result themselves. Returns an `Arc` clone
/// (cheap: an atomic refcount bump, no JNI call) rather than a real second
/// `Global`, since `Global` itself can't be cloned without an `Env` (see
/// the module-level note by `TABLE`).
pub fn resolve(handle: u64) -> Option<Arc<Global<JObject<'static>>>> {
    table().as_ref().unwrap().get(&handle).cloned()
}

/// Drops the table's entry for `handle`. The underlying `GlobalRef` is
/// released (via `Global`'s own `Drop` impl, which calls
/// `DeleteGlobalRef`) once every outstanding `Arc` clone handed out by
/// `resolve` has also been dropped -- this function does not itself touch
/// JNI.
pub fn release(handle: u64) {
    table().as_mut().unwrap().remove(&handle);
}

/// The Lisp-visible wrapper for a JVM object — a `user-ptr` under the
/// hood. Its `Drop` impl is what makes "wrap a JVM object as a Lisp
/// value" actually release the `GlobalRef` when Lisp's GC collects the
/// wrapper (spec: "Identity and lifetime").
pub struct JvmHandle(pub u64);

// `Transfer` is the `emacs` crate's marker trait for "expose this type to
// Lisp as an opaque, GC-owned `user-ptr`" (verified against the vendored
// emacs-0.21.0 source, `src/types/user_ptr.rs`) -- a bare impl is all it
// needs; there's no required method.
impl emacs::Transfer for JvmHandle {}

impl Drop for JvmHandle {
    fn drop(&mut self) {
        // Deliberately does NOT call `release(self.0)` directly here —
        // Lisp's GC may run this `Drop` on a thread that was never
        // JNI-attached, so the actual `GlobalRef` drop (which needs a
        // live JNI attachment) happens on the worker thread instead, via
        // `Cmd::ReleaseRef`'s handling in `jvm_thread::run` (Task 8),
        // which calls `handles::release` directly.
        let _ = crate::worker::send_command(crate::commands::Cmd::ReleaseRef { handle: self.0 });
    }
}

/// A handle registered against nothing real — no live JVM object needed
/// for the plumbing-only test in `jvm--test-make-handle`. Task 5 is what
/// makes handles wrap genuine JNI objects; this deliberately does not
/// touch `TABLE` (there's no real `Global` to store), just hands out a
/// fresh handle number so distinct calls produce distinct `JvmHandle`s.
#[cfg(feature = "test-support")]
pub fn register_dummy_for_test() -> u64 {
    NEXT_HANDLE.fetch_add(1, Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jni::vm::{InitArgsBuilder, JavaVM};
    use jni::JNIVersion;

    /// jni-rs 0.22.4's `JavaVM::new` does a racy check-then-create against
    /// its process-global singleton, with no synchronization of its own
    /// (`impl_with_libjvm`, vendored source): if this test's
    /// `JNI_CreateJavaVM` call collides with another JVM-creating test in
    /// this crate (`shim::tests`, `jvm_thread::tests`) running
    /// concurrently under `cargo test`'s default parallelism, the loser
    /// gets `Create(JniCall(AlreadyCreated))` even though a JVM now
    /// genuinely exists -- retrying picks up the winner's now-populated
    /// `JavaVM::singleton()` on the very next attempt.
    fn create_test_jvm() -> JavaVM {
        let mut last_err = None;
        for _ in 0..20 {
            let init_args = InitArgsBuilder::new().version(JNIVersion::V1_8).build().unwrap();
            match JavaVM::new(init_args) {
                Ok(vm) => return vm,
                Err(e) => {
                    last_err = Some(e);
                    // Give the winning thread's in-flight `JNI_CreateJavaVM`
                    // call (which can take a noticeable amount of wall time)
                    // room to finish and populate the singleton before the
                    // next attempt, instead of busy-looping into the same
                    // still-in-progress collision.
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
        panic!("failed to create/attach a test JVM after retries: {last_err:?}");
    }

    #[test]
    fn register_then_resolve_returns_the_same_object() {
        let vm = create_test_jvm();
        vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            let object_class = env.find_class(jni::jni_str!("java/lang/Object"))?;
            let obj = env.new_object(&object_class, jni::jni_sig!(() -> void), &[])?;
            let global = env.new_global_ref(&obj)?;
            let handle = register(global);
            assert!(resolve(handle).is_some());
            release(handle);
            assert!(resolve(handle).is_none());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn resolve_of_unknown_handle_is_none() {
        assert!(resolve(999_999).is_none());
    }
}
