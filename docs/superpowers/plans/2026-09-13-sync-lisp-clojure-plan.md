# Synchronous Lisp → Clojure calls (raw implementation)

Status: in progress (2026-09-13). Experimental. Additive to the existing
async worker; switch to replacing `jvm--call` only if a blocker forces it.

## Goal

A Lisp → Clojure (or any JVM) call that runs on the Emacs **main thread**:
attach to the already-running JVM, resolve + invoke, marshal the result back,
return — no worker round trip. Same-thread Clojure → Lisp callbacks made
*during* such a call run **directly**, without the async request queue.

Background/heavy work: Clojure launches it on its own threads and returns.
Clojure → Lisp from a **background JVM thread** stays async, on the existing
`lisp_requests` queue — unchanged. Emacs is single-threaded, so that path
cannot collapse.

"raw" = built on the published `emacs` 0.21 doc-hidden `unsafe Env::new(raw)` /
`env.raw()` directly. Do **not** depend on the new reentry API being added to
`~/Programming/projects/emacs-module-rs` (that is held for review).

## Key facts (verified in emacs 0.21 source)

- `Env` has **no lifetime**: `struct Env { raw: *mut emacs_env, protected }`
  (`env.rs:25`). The lifetime is on `Value<'e>`, which borrows `&'e Env`.
  So reconstruction is `unsafe { Env::new(raw) }` — no lifetime transmute.
- `Env::new` (`env.rs:34`) and `env.raw()` (`env.rs:51`) are `pub`, `#[doc(hidden)]`.
- Every defun call already does `Env::new(env)` fresh per invocation
  (`func.rs:71`), dropped on return. Nested Lisp→Clojure→Lisp→Clojure calls
  each mint their own `Env`. So the env stack is keyed to the **defun
  boundary**, one entry per sync-call entry. JNI natives are not module calls
  and mint no env — they reuse the innermost defun's env.
- `Env` has a `Drop` (`env.rs:137`) that unroots its `protected` `Value`s and
  does an unwind-protect save/clear/restore of any pending non-local exit.
  `Value::protect()` roots each value into *its own* env's list (`value.rs:48`).
  So a reconstructed per-native `Env` is the `Env`-level analogue of a JNI
  local frame: its temporaries are freed at its Drop. **Convert any result to
  a durable Java form (LispObject/make_global_ref) before that Env drops.**
- Two `Env` wrappers over one `emacs_env` each free only their own `protected`
  set — disjoint, no double-free. On an Emacs that fixed GC bug #31238,
  `protected` is `None` and Drop is a no-op (reconstruction is free).
- `Env::call`/`intern`/… **auto-clear** the non-local exit: `handle_exit`
  (`error.rs:142`) reads it, clears it, and returns `Err(ErrorKind::Signal
  {symbol,data})` / `Throw{tag,value}` with symbol/data as `TempValue`. So
  after a callback signals, the env is immediately reusable — no stale-exit
  hazard on the safe API. `TempValue::value(env)` is `unsafe` and must be
  rebound with the originating env (same thread here — fine).

## Design

### Sharing the JVM + classloader with the main thread
The worker owns `vm` and `loader` on its stack (`jvm_thread.rs`). The main
thread needs both. Mirror `worker::SENDER`:
- Store the `JavaVM` (Clone in this jni version — see `jvm_thread.rs:66`) in a
  global (`Mutex<Option<JavaVM>>` or `OnceCell`), set in `jvm_thread::run`
  after `create_jvm`, cleared on stop.
- Hoist the shared `loader` `Arc<Global<..>>` to a global too (like
  `handles::TABLE`), set on first `LoadJar`, so the sync path resolves
  jar-loaded classes the same way `handle_call` does.

### Main-thread attachment
Attach the main thread **as a daemon** (`attach_current_thread_as_daemon` —
verify exact jni 0.22 API) so it does not block `DestroyJavaVM` at `jvm--stop`.
Attach-once, stay attached (the main thread never exits mid-session);
`attach_current_thread` is a fast re-check when already attached.

### Reuse
Extract the core of `handle_call` (`jvm_thread.rs:672`) — context-classloader
set, class resolution via loader/`find_class`, `invoke_constructor`/
`invoke_method` — into a function callable from both the async worker and the
sync defun. Keep behaviour identical.

### Env stack
```rust
thread_local! {
    static ENV_STACK: RefCell<Vec<*mut emacs::raw::emacs_env>> =
        const { RefCell::new(Vec::new()) };
}
struct EnvScope; // push env.raw() at jvm--call-sync entry, pop on Drop (return + unwind)
```
A background JVM thread's stack is empty → routes to the existing async queue.

### Reentrant callback fast-path (shim.rs `Lisp.*` natives)
Each native (`apply`, `intern`, `typeOf`, `isNotNil`, `eq`, `prin1`; `freeRef`
stays queue-only — it can run on a thread with no env): if `ENV_STACK` top
exists, `let env = unsafe { Env::new(raw) };`, run the op directly, marshal to
durable Java before `env` drops; else fall back to enqueue + block for reply
(existing path). Reuse `lisp_requests`' arg/result conversion helpers.

### Boundary unwinding
- Inbound (native): `env.call(..)` → `Err(ErrorKind::Signal/Throw)` (exit
  already cleared) → build `LispSignal`/`LispThrow` Java object (existing shim
  classes) → throw, return `Err(JavaException)`.
- Outbound (`jvm--call-sync`): Clojure returned exceptionally → clear JNI
  exception → if it unwraps to a `LispSignal`/`LispThrow`, reconstruct sym/data
  (same `borrow(env, ptr)` deref as `lisp_requests.rs:100`) and re-raise via
  `env.signal(..)` / throw; else `env.signal(jvm_error, msg)`.
- Invariant at every hop: do exactly one of {read-then-clear, propagate};
  never touch that env/JNI side again while pending.

## unsafe inventory
1. `unsafe { Env::new(raw) }` per native — contract "raw is a live emacs_env",
   upheld by same-thread + deeper-on-stack + `EnvScope` pop-before-return.
2. `env.raw()` at push — safe, plain read.
3. `TempValue::value(env)` rebinding signal sym/data — same-env requirement met.
4. `&*(ptr as *const GlobalRef)` / `Box::from_raw` for LispObjects a signal
   carries — existing `lisp_requests.rs:100` pattern.
5. JNI FFI / `RegisterNatives` — existing surface.

## Phases (TDD; `cargo test --lib -- --test-threads=1` + `./test/run-ert-tests.sh`)
1. [DONE] Share `JavaVM` + `loader` globally (`runtime`); refactor
   `resolve_and_invoke` out of `handle_call`.
2. [DONE] `jvm--call-direct` defun + `jvm-call-direct`/`jvm-new-direct` elisp
   wrappers: attach, resolve+invoke, marshal, return; `jvm-error` on failure.
   ERT `test/jvm-call-direct-test.el` (7 tests). Additive — worker/`jvm--call`
   untouched.
3. [DONE] Env stack (`reentry`: thread-local + `EnvScope`) wired into
   `jvm--call-direct`.
4. [DONE] Reentrant fast-path in the `Lisp.*` natives + `applyAsync`
   (`shim.rs`). ERT `test/jvm-clj-direct-test.el` (4 tests): same-thread
   Clojure→Lisp callback runs without the queue (would otherwise deadlock).
   Background-thread callback still uses the queue (all async tests still pass).
5. [DONE] Boundary unwinding, both directions. Inbound: a Lisp signal/throw in
   a reentrant callback is thrown as a `LispNonLocalExit` up the Clojure stack
   (`throw_non_local_exit`). Outbound: when such an exit reaches the top of a
   direct call, `describe_pending_java_exception` (gated on `ATTACH_DEPTH > 0`)
   extracts the `LispNonLocalExit`'s fields via `shim::object_to_lisp_request_arg`
   into a `DirectError::NonLocalExit`; `jvm--call-direct` rebinds them and
   re-raises the *original* `(signal …)`/`(throw …)`. A genuine Clojure/Java
   exception still surfaces as `jvm-error`. ERT: `reentrant-lisp-signal-
   propagates-out` (asserts symbol `error` + data), `reentrant-lisp-throw-
   propagates-out` (caught by an enclosing `catch`).
6. [DONE] Docs: this plan updated; design spec
   `docs/superpowers/specs/2026-09-13-sync-lisp-clojure-design.md`; README;
   `docs/explanation/limitations-and-tradeoffs.md`; elisp docstrings; dense
   code doc-comments throughout.

## Deviations from the original design notes
- Named `jvm--call-direct`/`jvm-call-direct` (not `-sync`): `jvm-call-sync`
  already exists (async `jvm-call` + a blocking wait on the worker).
- No daemon attach: jni 0.22 exposes none, and `DestroyJavaVM` blocks on a
  live non-daemon attachment. So the main thread attaches permanently but
  `call_direct` detaches when its outermost frame returns (thread-local depth
  counter) — the "attach, execute, detach, return" shape, with nested
  reentrant calls sharing the one attachment.

## Follow-ups
- Consider adopting the reviewed emacs-module-rs reentry API in place of the
  raw `unsafe Env::new(raw)` once the user approves it.

## Out of scope
- Wiring emacs-jni to the new emacs-module-rs reentry API (held for review).
- Removing the worker / async `jvm--call` (only if a blocker forces replace).
