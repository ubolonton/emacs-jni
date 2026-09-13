# Synchronous Lisp → Clojure calls — design

Experimental. Additive to the async worker (`jvm-call`/`jvm-call-sync`). See
the plan `docs/superpowers/plans/2026-09-13-sync-lisp-clojure-plan.md` for
phase status and follow-ups.

## Goal

A Lisp → JVM call that runs on the Emacs **main thread**: attach to the
running JVM, resolve + invoke, return the value, detach — no worker thread,
no channel round trip. Same-thread Clojure → Lisp callbacks made *during* such
a call run directly, not over the async request queue.

Public entry: `jvm-call-direct` (and `jvm-new-direct`). The primitive is
`jvm--call-direct`. It returns the value directly and signals `jvm-error` on
failure — distinct from `jvm-call-sync`, which is the async `jvm-call` plus a
blocking wait on the worker's `call-done` reply.

## Core constraint

Emacs is single-threaded. During `jvm-call-direct` the main thread is blocked
inside the JVM for the whole call. So:

- Heavy or blocking work must stay on Clojure's own threads (the call returns
  once the direct work is done).
- A Clojure → Lisp callback from a **background** JVM thread still uses the
  async queue (`lisp_requests`) — unchanged. It cannot collapse, because the
  main thread is single and, mid-`jvm-call-direct`, busy.
- A Clojure → Lisp callback on the **same** (main) thread, during the call,
  must run **directly**: enqueuing it would deadlock, since draining the queue
  needs the main thread back in its Emacs message loop.

## JVM sharing and attachment

The worker owns the `JavaVM` and the shared jar classloader. `runtime` mirrors
both into process globals so the main thread can attach and resolve jar
classes exactly as `jvm_thread::resolve_and_invoke` does (shared with the async
`handle_call`).

jni 0.22 has no daemon attach, and `DestroyJavaVM` blocks on any live
non-daemon attachment. So `call_direct` attaches the main thread permanently
(cheap re-check when already attached) but **detaches when its outermost frame
returns** — tracked by a thread-local depth counter. Between top-level direct
calls the main thread is detached, so `jvm-stop`'s `destroy` never blocks;
nested reentrant calls share the one attachment.

## Reentrancy: the Env stack

`emacs` 0.21's `Env` is a bare pointer wrapper (no lifetime); each defun call
mints its own, dropped on return. A `Lisp.*` native is not a module call, so it
has no `Env` of its own — but during `jvm-call-direct` the enclosing defun's
`Env` is still live on the same thread's C stack.

`reentry` keeps a thread-local stack of those live `emacs_env` pointers, pushed
by an `EnvScope` RAII guard at `jvm--call-direct` entry (one entry per frame;
Lisp→Clojure→Lisp→Clojure nests). A `Lisp.*` native, when the stack is
non-empty, reconstructs a scoped `&Env` for the innermost entry via the
published 0.21 `unsafe Env::new(raw)` and runs the op directly.

Soundness of that `unsafe`: same thread, deeper on the stack than the
`EnvScope` that pushed the pointer (so it is live, not yet popped), and the
reconstructed `Env` is never stored past the native call. Its `Drop` frees the
`Value`s it rooted (the 0.21 GC-bug workaround), so it doubles as a per-native
cleanup frame — results are returned as `LispReplyValue` (whose `Ptr` variant
holds an independent `GlobalRef`), which does not borrow it.

## Request handling, shared with the async path

`lisp_requests::run_request_value` runs the actual `Env` op for every
reply-bearing request; `service` (async, worker replies over the channel) and
`compute_reply` (direct) both use it. `jvm_thread::complete_future_with_reply`
and `build_lisp_reply_object` build the Java result, shared by the worker's
future completion and the direct `applyAsync` fast path.

The `Lisp.*` sync natives (`intern`, `typeOf`, `isNotNil`, `eq`,
`prin1ToString`) return the built object directly. `applyAsync` returns an
*already-completed* future, so Clojure's `(deref (applyAsync ...))` resolves at
once. `FreeRef` stays queue-only (its `Cleaner` thread pushes no scope).

## Boundary unwinding

- Inbound (implemented): a Lisp signal/throw raised in a reentrant callback is
  built into a `LispNonLocalExit` and thrown (`throw_non_local_exit`), so it
  propagates up the Clojure stack exactly like the async path's rethrow. The
  `emacs` safe API auto-clears the pending exit (`handle_exit`), so there is no
  stale-exit hazard.
- Outbound (not implemented): a Clojure exception — including an uncaught
  `LispNonLocalExit` — surfaces at `jvm--call-direct` as a generic `jvm-error`.
  Reconstructing the original Lisp signal symbol/data is a follow-up.

## Components

- `src/runtime.rs` — shared `JavaVM` + classloader globals.
- `src/reentry.rs` — Env-pointer stack, `EnvScope`, `with_current_env`.
- `src/jvm_thread.rs` — `resolve_and_invoke`, `call_direct`, and the shared
  reply/exception builders.
- `src/lisp_requests.rs` — `run_request_value`, `compute_reply`,
  `lisp_arg_to_value`.
- `src/shim.rs` — the reentrant fast path in the `Lisp.*` natives.
- `src/lib.rs` — the `jvm--call-direct` defun; `elisp/jvm.el` wrappers.

## Testing

- `test/jvm-call-direct-test.el` — forward calls (static/instance, nil→false,
  bignum, overload, error paths), parity with `jvm-call-sync`.
- `test/jvm-clj-direct-test.el` — reentrant Clojure→Lisp (sync natives, apply,
  require-symbol, signal-propagates) with no deadlock; gated behind
  `EMACS_JNI_CLOJURE_JARS_DIR` + `test-support`.

## Risks and tradeoffs

- A long direct call freezes Emacs's main loop for its duration.
- Attach/detach per top-level direct call has a small cost (no daemon attach).
- The reentrant `unsafe Env::new(raw)` rests on the same-thread /
  deeper-on-stack invariant; a Clojure callback deferred to a background thread
  correctly falls back to the async queue (its thread's Env stack is empty).
