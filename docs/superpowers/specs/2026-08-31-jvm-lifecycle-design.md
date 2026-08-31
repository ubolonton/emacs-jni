# Phase 1 Design: JVM Lifecycle and Jar Loading

Status: approved for planning
Date: 2026-08-31

## Purpose

Embed a JVM inside an Emacs dynamic module, as the foundation for a larger
project (Emacs <-> JVM object interop, then Clojure eval + nREPL). This spec
covers **only** the first phase: getting a JVM running under Emacs control,
with lifecycle management and jar loading. It does not cover object
marshalling, cross-runtime method calls, or Clojure.

## Project decomposition

The overall project is split into three phases, each with its own
spec -> plan -> implementation cycle:

1. **Phase 1 (this spec):** JVM lifecycle (start/pause/stop/status) and jar
   loading, running the JVM on a dedicated thread that talks back to the
   Lisp thread via `emacs-module-rs`'s `Env::open_channel`.
2. **Phase 2:** Bidirectional JVM <-> Lisp interop — wrap a Lisp value as a
   JVM object, wrap a JVM object as a Lisp value, JVM calls a Lisp function,
   Lisp calls a JVM method. Assumes no cycles in the cross-runtime object
   graph.
3. **Phase 3:** Clojure — evaluate Clojure code, run an nREPL server other
   processes can connect to.

## Target platform

Emacs 28+ (required by `open_channel`), Linux only for now. macOS/Windows
support (the latter requiring CRT-matched linking via MSYS2 MINGW64) is
deferred.

## Crate choices

- `emacs` (emacs-module-rs), with the `emacs-28` feature, for the dynamic
  module scaffolding (`#[module]`, `#[defun]`) and `Env::open_channel`.
- `jni` (jni-rs), **not** j4rs. jni-rs gives direct control over:
  attaching/detaching arbitrary OS threads to the JVM, `GlobalRef`/
  `WeakGlobalRef` lifetime management, defining native methods on a custom
  Java interface, and reflective classloader manipulation. Phase 2 needs all
  of this for object wrapping and native-method callbacks; j4rs's
  higher-level abstraction would need to be worked around for exactly the
  parts this project needs most, so building on jni-rs from the start avoids
  a later rewrite.

Single crate (not a Cargo workspace) for Phase 1. Revisit a workspace split
if Phase 2 needs a separate build step (e.g. a generated shim jar).

## Architecture

The JVM runs on a dedicated OS thread, spawned by `jvm-start` and torn down
(or exited, for pause) as part of the lifecycle below. All JNI calls happen
on this thread; the Lisp (Emacs main) thread never touches JNI directly.

Two communication paths, one in each direction:

- **Lisp thread -> JVM thread:** an in-process Rust `mpsc` channel
  (`Sender<Cmd>` held by the module, `Receiver<Cmd>` owned by the JVM
  thread's command loop). `#[defun]`s push a command and return immediately
  — this is plain Rust inter-thread communication, no Emacs API involved.
- **JVM thread -> Lisp thread:** `Env::open_channel`, opened once against a
  process object Elisp creates via `make-pipe-process`. The returned
  `impl Write + Send + Sync` is handed to the JVM thread; it writes
  newline-terminated S-expressions, which Emacs delivers to the pipe
  process's filter function on the main thread.

```
Lisp thread (Emacs main)                    JVM thread (dedicated, spawned by jvm-start)
-------------------------                   --------------------------------------------
(jvm-start)  --[mpsc Sender<Cmd>]-------->   recv loop:
(jvm-pause)  --[Cmd::Pause]-------------->     Cmd::Start(opts) => JavaVM::new(...) via jni-rs,
(jvm-stop)   --[Cmd::Stop]---------------->      write "(status starting)"/"(status ready)"/
(jvm-load-jar path) --[Cmd::LoadJar(id,path)]->   "(status start-failed ...)" to channel writer
                                                Cmd::Pause => detach thread, keep JavaVM* alive,
                                                  write "(status paused)", thread exits
                                                Cmd::Stop => attempt JavaVM::destroy() (may hang;
                                                  known JNI caveat, see Risks), write
                                                  "(status stopped)"/"(status stop-failed)"
                                                Cmd::LoadJar => reflective addURL on shared
                                                  URLClassLoader, write "(load-jar-done ID t)"/
                                                  "(load-jar-done ID nil ERR)"

              <--[open_channel pipe process]-- s-expression lines, newline-terminated

Elisp glue (process filter on the pipe process):
  accumulate bytes -> split on newline -> (read line) -> dispatch on tag:
    (status STATE)             => setq jvm--status STATE, run-hook-with-args 'jvm-status-hook STATE
    (load-jar-done ID OK ERR)  => resolve pending callback for ID, run hook
  jvm-status is just (symbol-value 'jvm--status) -- instant, no Rust call
```

Command IDs (a small integer counter, module-global) correlate async
`load-jar` requests with their `load-jar-done` events, since multiple loads
can be in flight concurrently.

### Pause vs. stop

JNI has a well-known caveat: destroying a JVM (`JNI_DestroyJavaVM`) and then
creating a new one in the same OS process is unsupported/unreliable on many
JVM builds (can hang or crash). We expose both operations rather than
picking one:

- **`jvm-pause`:** the JVM thread finishes its command loop and exits
  (dropping its JNI thread attachment), but the module keeps the `JavaVM`
  handle alive in a global. `jvm-start` after a pause spawns a *new* thread
  that calls `AttachCurrentThread` on the existing `JavaVM` rather than
  creating a new one. This is the safe, restart-capable path.
- **`jvm-stop`:** attempts a real `JNI_DestroyJavaVM` teardown. This is
  explicitly experimental — `jvm-start` after a `jvm-stop` is "try it and
  see," not a supported guarantee. Runs on its own throwaway thread so a
  hang doesn't block Emacs (it does leak that thread forever if the destroy
  call truly never returns — accepted for a prototype).

### State machine

States: `not-started`, `starting`, `ready`, `pausing`, `paused`, `stopping`,
`stopped`, `start-failed`, `stop-failed`, `crashed`.

Legal transitions (enforced both in Elisp, for UX, and via a Rust-side
`AtomicU8` mirror checked inside each `#[defun]`, so illegal calls signal a
Lisp error immediately instead of being silently queued):

- `not-started` / `paused` / `stopped` / `crashed` / `start-failed` ->
  (`jvm-start`) -> `starting` -> `ready` | `start-failed`
- `ready` -> (`jvm-pause`) -> `pausing` -> `paused`
- `ready` / `paused` -> (`jvm-stop`) -> `stopping` -> `stopped` |
  `stop-failed`
- any state with a live command loop -> panic -> `crashed` (best-effort
  `(status crashed "...")` event, thread exits)
- `ready` -> (`jvm-load-jar`) does not change the top-level status; tracked
  per-request via `load-jar-done`

The transient states (`starting`, `pausing`, `stopping`) are set
synchronously by the Elisp wrapper function itself, at the moment it issues
the command — not read back from the channel. This is what the async
`#[defun]`s actually update on the Lisp side before returning: e.g.
`jvm-start` validates the current status, sets `jvm--status` to `starting`
immediately, then calls the Rust primitive that sends `Cmd::Start` and
returns. This matters for correctness: the sync wrappers loop on
`jvm-status`, and if the transient state were only set later by an event
from the JVM thread, a loop like `(while (eq (jvm-status) 'starting) ...)`
could race and exit immediately, before the command has even been picked
up. Only the terminal states (`ready`, `paused`, `stopped`, `start-failed`,
`stop-failed`, `crashed`) arrive asynchronously via the channel. The
Rust-side atomic mirror is updated at the same two points (synchronously
when the command is sent, and again from the JVM thread when a terminal
state is reached), so `#[defun]` gating stays consistent with what Elisp
sees.

## Jar loading

All `jvm-load-jar` calls add to a single shared `URLClassLoader` (via
reflective `addURL`, since the JDK 26 system classloader is no longer a
`URLClassLoader` subclass and doesn't support post-construction additions).
Every loaded jar can see every other loaded jar — this matches real
dependency graphs (e.g. a Clojure jar plus AOT-compiled libraries plus user
jars all need to see each other).

Consequence: there is no true per-jar unload in Phase 1. `jvm-unload-jar` is
out of scope for this phase (the "if possible" unload from the original
ask); the only way to clear loaded jars is `jvm-pause` or `jvm-stop`, which
drop the whole classloader. This can be revisited later (e.g. a LIFO
classloader chain) if per-jar unload becomes a real requirement.

## Lisp-facing API

Config (plain `defvar`, not `defcustom`):
- `jvm-classpath` — list of paths, read at `jvm-start` time.
- `jvm-options` — list of JVM flag strings (e.g. `"-Xmx512m"`), read at
  `jvm-start` time.

Async primitives (Rust `#[defun]`s, return immediately):
- `jvm-start (&optional classpath options)`
- `jvm-pause ()`
- `jvm-stop ()`
- `jvm-load-jar (path &optional callback)` -> integer request-id;
  `callback`, if given, is called as `(callback ok err)` when
  `load-jar-done` arrives for that id.
- `jvm-status ()` -> reads `jvm--status`, an Elisp variable set by the
  process filter. No Rust call, no round trip.

Observability:
- `jvm-status-hook` — run with the new state symbol on every transition.
- `*jvm-log*` buffer, fed by any `(log ...)` channel messages.

Sync convenience wrappers (pure Elisp, built on the async primitives and
`accept-process-output`):
- `jvm-start-sync`, `jvm-pause-sync`, `jvm-stop-sync`, `jvm-load-jar-sync`,
  each taking an optional timeout. Pattern: call the async function, then
  loop `(accept-process-output jvm--pipe-process 0.1)` until the relevant
  state/request resolves or the timeout signals an error.

## Wire protocol

Newline-terminated, hand-written S-expressions — no serde/json dependency,
since Elisp reads these natively via `read` and the message shapes are
small and fixed:
- `(status STATE)`
- `(status start-failed "message")` / `(status stop-failed "message")` /
  `(status crashed "message")`
- `(load-jar-done ID OK ERR)` — `OK` is `t`/`nil`, `ERR` is a string or nil
- `(log LEVEL "message")`

## Error handling

- The JVM thread's command loop runs under `catch_unwind`. On panic, it
  best-effort writes `(status crashed "...")` (if the channel writer is
  still valid) and exits.
- `jvm-start` from `crashed` is allowed: it spawns a fresh thread and tries
  `AttachCurrentThread` on the retained `JavaVM` handle. This may itself
  fail if the crash corrupted JVM-internal state, in which case it reports
  `start-failed` again rather than looping.
- Every `#[defun]` checks the Rust-side atomic status before sending a
  command, rejecting illegal transitions synchronously with a Lisp error.

## Testing

- `cargo test` for pure Rust logic (state machine transitions, message
  formatting) — no Emacs required.
- Integration tests via `emacs --batch` + ERT driving the built `.so`:
  `jvm-start-sync` -> `jvm-load-jar-sync` (against a tiny fixture jar built
  with `javac`, since `mise.toml` already pins Java 26) -> `jvm-pause-sync`
  -> `jvm-start-sync` (resume) -> `jvm-stop-sync`, asserting status
  transitions at each step.

## Risks / known limitations (accepted for this phase)

- `jvm-stop` relies on `JNI_DestroyJavaVM`, which is not reliably supported
  across JVM builds and can hang. Mitigated by running it on a throwaway
  thread and treating `jvm-start` after `jvm-stop` as unsupported/
  experimental; not otherwise worked around.
- Jar "unloading" is not real class unloading — Phase 1 has no per-jar
  unload at all, only whole-JVM pause/stop.
- Only one JVM instance exists per Emacs process (a JNI-level constraint,
  not a design choice) — not relevant unless multiple concurrent JVMs are
  ever needed, which is out of scope.

## Out of scope for Phase 1

- Any JVM <-> Lisp value/object marshalling (Phase 2).
- Any JVM-initiated call into a Lisp function (Phase 2).
- Clojure evaluation, nREPL (Phase 3).
- Per-jar unload (see Risks).
- macOS/Windows support.
