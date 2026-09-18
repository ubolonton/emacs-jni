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
        // reference is cloned and handed to the worker thread to release:
        // in the common case, dropping the last `Arc` there runs
        // `Global::Drop`'s `DeleteGlobalRef` on an attached thread, avoiding
        // `jni::Global::Drop`'s GC-thread auto-attach and its logged perf
        // warning (jni-0.22.4, `src/refs/global.rs`). This is best-effort,
        // not a guarantee: if the worker drops its clone before the field
        // drop below runs, this (GC) thread ends up holding the last `Arc`
        // and pays the auto-attach cost itself. Either way the delete runs
        // exactly once -- never a double-free, never a leak.
        let object = self.0.clone();
        let _ = crate::worker::send_command(crate::commands::Cmd::ReleaseRef { object });
    }
}
