//! Same-thread reentrancy support for `jvm--call-direct` (plan
//! `docs/superpowers/plans/2026-09-13-sync-lisp-clojure-plan.md`, phases 3-5).
//!
//! A synchronous `jvm--call-direct` runs Clojure on the Emacs main thread and
//! blocks there for the whole call. If that Clojure code calls back into Lisp
//! (`emacs-jni.clj/apply`, `intern`, ...), the ordinary async path would
//! deadlock: it enqueues a `lisp_requests::Inbound` and waits for the main
//! thread to drain the queue, but the main thread is the one blocked inside
//! the JVM. So the `Lisp.*` natives must instead run the op *directly*, on the
//! still-live `Env` of the enclosing `jvm--call-direct` frame.
//!
//! This module keeps the stack of those live `Env`s, keyed to the defun
//! boundary (one entry per `jvm--call-direct` frame; see `emacs` 0.21
//! `func.rs`, which mints a fresh `Env` per defun call). A `Lisp.*` native is
//! not itself a module call, so it reuses the innermost frame's `Env`.
//!
//! A background JVM thread never pushes here, so its stack is empty and the
//! natives fall back to the async queue -- exactly as before.

use std::cell::RefCell;

use emacs::Env;

thread_local! {
    /// Raw `emacs_env` pointers for the in-flight `jvm--call-direct` frames on
    /// this thread, innermost last. Stored as raw pointers (Copy, no
    /// lifetime): validity is a dynamic-extent invariant upheld by `EnvScope`
    /// (pushed on entry, popped on return *and* unwind), not by the borrow
    /// checker.
    static ENV_STACK: RefCell<Vec<*mut emacs::raw::emacs_env>> = const { RefCell::new(Vec::new()) };
}

/// RAII marker: records ENV's raw pointer for the duration of a
/// `jvm--call-direct` frame, and removes it on drop (normal return or panic
/// unwind), so a stale pointer never outlives the module call it names.
#[must_use = "the Env pointer is popped when this guard drops; bind it to a local"]
pub struct EnvScope {
    _private: (),
}

/// Pushes ENV's raw pointer and returns the guard that pops it. Call once, at
/// the top of the `jvm--call-direct` defun.
pub fn enter(env: &Env) -> EnvScope {
    ENV_STACK.with(|stack| stack.borrow_mut().push(env.raw()));
    EnvScope { _private: () }
}

impl Drop for EnvScope {
    fn drop(&mut self) {
        ENV_STACK.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

/// Whether this thread is currently inside a `jvm--call-direct` frame -- i.e.
/// a `Lisp.*` native running now must take the direct path, not the queue.
pub fn in_scope() -> bool {
    ENV_STACK.with(|stack| !stack.borrow().is_empty())
}

/// Runs F with a reconstructed `&Env` for the innermost live `jvm--call-direct`
/// frame on this thread, or returns `None` if there is none.
///
/// The reconstructed `Env` is scoped to F: `emacs` 0.21's `Env` roots the
/// `Value`s it creates in its own list and frees them on `Drop` (the GC-bug
/// workaround), so it acts as a per-native cleanup frame -- convert any result
/// to a form that does not borrow it (a `LispOutcome`, whose `Ptr` variant
/// already holds an independent `GlobalRef`) before this returns.
pub fn with_current_env<R>(f: impl FnOnce(&Env) -> R) -> Option<R> {
    let raw = ENV_STACK.with(|stack| stack.borrow().last().copied())?;
    // SAFETY: `raw` is the innermost still-in-flight `jvm--call-direct` env.
    // The native calling this runs deeper on the SAME thread's stack than the
    // `EnvScope` that pushed it, so the pointer is live (not yet popped). The
    // reconstructed `Env` is used only within F, inside that dynamic extent,
    // and never stored. `Env::new`'s only contract is that `raw` is a live
    // `emacs_env`, which holds here. See this module's doc comment.
    let env = unsafe { Env::new(raw) };
    Some(f(&env))
}
