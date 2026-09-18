use std::sync::Arc;

use jni::objects::JObject;
use jni::refs::Global;

/// A live global reference to a JVM object, shared by `Arc` clone.
///
/// Replaces the former `u64` handle plus process-global table. The table
/// only existed so a JVM object could be a text-serialisable integer over
/// the old sexp pipe (deleted in Item 2). `jni::refs::Global` is `Send +
/// Sync` (0.22.4), so the reference itself rides the typed `Cmd`/`Inbound`
/// queues directly. `Arc` because 0.22's `Global` has no `Clone` (a real
/// clone is a fresh `NewGlobalRef`, needing a live `Env`); an `Arc` clone
/// is a cheap refcount bump.
#[derive(Debug, Clone)]
pub struct JvmObject(pub Arc<Global<JObject<'static>>>);

/// The Lisp-visible wrapper for a JVM object — a `user-ptr`. Exactly one
/// wrapper owns a given reference (mirrors `LispObject`'s single-owner
/// rule): each JVM→Lisp crossing mints a fresh `Global`, and the wrapper
/// is neither cloned nor copied.
pub struct JvmHandle(pub JvmObject);

// `Transfer` is the `emacs` crate's marker for "expose as an opaque,
// GC-owned `user-ptr`" (vendored emacs-0.21.0, `src/types/user_ptr.rs`) —
// a bare impl, no required method.
impl emacs::Transfer for JvmHandle {}

impl Drop for JvmHandle {
    fn drop(&mut self) {
        // Lisp's GC may run this on a thread with no JNI attachment, so the
        // reference is handed to the worker thread to release: dropping the
        // last `Arc` there runs `Global::Drop`'s `DeleteGlobalRef` on an
        // attached thread. `jni::Global::Drop` would otherwise auto-attach
        // the GC thread with a logged perf warning (jni-0.22.4,
        // `src/refs/global.rs`). Cloning (not moving) the `Arc` keeps a
        // reference alive past this frame, so the field's own drop below
        // never triggers `DeleteGlobalRef` on this (GC) thread.
        let object = self.0.clone();
        let _ = crate::worker::send_command(crate::commands::Cmd::ReleaseRef { object });
    }
}
