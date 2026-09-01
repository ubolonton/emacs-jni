# Phase 2 Design: Bidirectional JVM ↔ Lisp Interop

Status: draft — transport, lifecycle, threading model, calling convention,
value marshalling, and error propagation all settled. Pending: a holistic
review of this revision before moving to `writing-plans`.

Date: 2026-09-01

## Purpose

Phase 2's overall scope (per `HANDOFF.md` / `README.md`'s roadmap) is four
tightly-coupled deliverables: wrap a Lisp value as a JVM object, wrap a JVM
object as a Lisp value, JVM calls a Lisp function, Lisp calls a JVM method.
All four sit on top of one shared foundation: a message-passing protocol
between the JVM worker thread(s) and the Lisp thread(s), plus the lifetime
mechanics for values that cross the runtime boundary. This document covers that foundation, plus the calling convention built on
top of it: what Java-side interface a wrapped Lisp function implements, how
values marshal between the two runtimes, and how errors on either side
surface on the other.

## Design principle: async transport, no blocking in the protocol layer

Every cross-runtime call — either direction — is a message tagged with a
correlation ID, sent over the existing (Phase 1) channel machinery, with no
expectation of an immediate reply. Nothing in the protocol itself ever
blocks a thread waiting for a specific response. Where a synchronous-feeling
API is useful (e.g. a `-sync` convenience wrapper, matching Phase 1's
`jvm-load-jar-sync`), it is a thin polling layer built *on top of* the async
primitive — never a transport-level requirement.

This directly follows from a deliberate scope cut: **Phase 2 does not
support wrapping a Lisp function to satisfy a pre-existing synchronous Java
interface** (`java.util.Comparator`, `Runnable`, `Function`, event listener
interfaces, etc.). Those interfaces have no async escape hatch by
construction — the calling Java code needs a real return value on the spot
— which would force some JVM thread to block for the round-trip to Lisp and
back. Ruling this out means no thread, on either side, ever blocks inside
the interop protocol itself. (The actual Java-side interface Phase 2
defines — necessarily async-native in shape — is in "Calling convention"
below; this section only fixes the constraint it must satisfy.)

One consequence worth stating plainly: because nothing blocks, there is no
concept of "the thread waiting for this reply" anywhere in the core
protocol, on either side. A reply is delivered to whatever thread happens to
process it, which looks up the correlation ID in a shared table and acts —
full stop. Both threading sub-sections below follow from this.

## Wire protocol extension

Extends Phase 1's existing S-expression protocol (`src/protocol.rs`,
`src/commands.rs`) with new tags, following the same shape as the existing
`status`/`load-jar-done`/`log` messages. `ARGS`/`RESULT` (and the
equivalent `Cmd` fields) carry values converted per "Value marshalling"
below.

Worker → Lisp (`channel::send`, new tags alongside `status`/`load-jar-done`/`log`):

- `(call-done ID OK RESULT ERR)` — result of a Lisp-initiated JVM call
  (`Cmd::Call`). Same OK/ERR shape as `load-jar-done`.
- `(invoke ID HANDLE ARGS)` — the JVM, via a wrapped-Lisp-value proxy object,
  asking Lisp to run the Lisp value identified by `HANDLE`.
- `(release HANDLE)` — the JVM-side proxy object for `HANDLE` was garbage
  collected; Lisp may drop its corresponding entry.

Lisp → worker (new `Cmd` variants, same `mpsc` channel as `Cmd::Pause`/`Cmd::Stop`/`Cmd::LoadJar`):

- `Cmd::Call { id, target, member, args }` — Lisp asks the JVM to invoke a
  method. Construction (`jvm-new`, see "Calling convention") reuses this
  same variant with `member` set to `"<init>"` — the constructor's actual
  internal name in JVM bytecode — rather than inventing a parallel `Cmd`/
  wire-message pair; the worker's handler special-cases that value to use
  `getConstructor`/`newInstance` instead of `getMethod`/`invoke`.
- `Cmd::InvokeReply { id, result }` — Lisp's answer to an `(invoke ...)`
  request.
- `Cmd::ReleaseRef { handle }` — mirror of `release`: a Lisp-side `user-ptr`
  wrapping a JVM object was garbage collected.

## Identity and lifetime: two independent handle tables

Each side owns exactly the table it can resolve; neither side needs to hold
a reference into the other runtime's memory.

- **Lisp owns "which Lisp value is `HANDLE`."** A plain hash table mapping
  an integer handle to the wrapped Lisp function/value, exactly like the
  existing `jvm--pending-load-jar` alist maps request IDs to callbacks.
  Rust never holds a reference to the Lisp value at all — it only ever
  hands the JVM-side proxy an opaque integer, and relays `invoke`/
  `InvokeReply` messages tagged with it. When the JVM-side proxy for a
  given `HANDLE` is GC'd, a `(release HANDLE)` message lets Lisp drop the
  entry (the proxy uses a Java `Cleaner` or equivalent to detect its own
  collection and notify Rust, which forwards the message).
- **Rust owns "which JVM object is `HANDLE`."** A `HashMap<handle,
  GlobalRef<JObject>>`, populated when a JVM object is wrapped as a Lisp
  value. The Lisp-side representation is a `user-ptr` carrying the handle,
  with a finalizer that fires `Cmd::ReleaseRef { handle }` when Lisp's GC
  collects it, so Rust can drop the `GlobalRef`. If the high-level `emacs`
  crate doesn't expose `user-ptr` finalizer attachment, fall back to the
  lower-level `emacs-module` crate's raw bindings (see "Crate dependency
  policy" under "Value marshalling") rather than blocking on it.

This reuses Phase 1's existing "no cycles in the cross-runtime object
graph" assumption (stated in the original three-phase decomposition) —
independent per-side reference tracking is exactly what breaks down if a
Lisp value and a JVM object end up holding live references to each other.
With the calling convention below now concrete, one mechanism is worth
naming plainly rather than leaving abstract: a Lisp closure that captures a
wrapped JVM object, where that same JVM object holds (directly or
transitively) a reference back to a `LispFunction` wrapping that closure,
is exactly the shape of cycle neither side's local GC can see across the
boundary to collect — each side's "is this still reachable" answer depends
on the other side's, and nothing traces the whole picture. This is the same
"no cycles" assumption stated in the original three-phase decomposition,
now with a concrete example of what violating it looks like. Still an
accepted limitation, not solved here — see "Out of scope."

## Calling convention

### Java shim

A small `.java` source, compiled via `javac` in a `build.rs` step and
embedded into the binary with `include_bytes!` (injected into the running
JVM at startup via `JNIEnv::DefineClass` — no jar file to locate at
runtime), defines the minimum surface needed for "wrap a Lisp value as a
JVM object":

```java
public interface LispCallable {
    CompletableFuture<Object> call(Object... args);
}

final class LispFunction implements LispCallable {
    private final long handle;
    LispFunction(long handle) { this.handle = handle; }
    public native CompletableFuture<Object> call(Object... args);
}

final class LispError extends RuntimeException {
    LispError(String message) { super(message); }
}
```

`LispFunction.call`'s native implementation follows the async-transport
principle exactly: allocate a fresh correlation id, construct a
`CompletableFuture<Object>` via JNI, register its `GlobalRef` in the
pending-continuations table under that id, post `(invoke ID HANDLE ARGS)`
(`HANDLE` = `this.handle`) via `channel::send`, and return the
still-incomplete future immediately. Nothing here ever blocks a JVM thread.

### Lisp-facing primitives

- `jvm-call (target member &rest args)` — `target` is a string (fully-
  qualified class name) for a static call, or a wrapped-JVM-object
  `user-ptr` for an instance call; dispatches on which kind of Lisp value
  `target` is. Async by default, with `-sync`/callback variants following
  the existing `jvm-load-jar`/`jvm-load-jar-sync` convention.
- `jvm-new (class-name &rest args)` — construction, kept as a separate
  *Lisp-facing* primitive rather than overloading `jvm-call`'s target
  further (a class name there reads as "call a static method," not
  "construct one"), even though underneath it sends the identical
  `Cmd::Call` wire message with `member` set to `"<init>"` (see "Wire
  protocol extension").

### Dispatch mechanism

Reflection-based: `Class.getMethod`/`getMethods` (scanning for a best
arg-count/assignability match when overloads exist) + `Method.invoke`, or
`getConstructor`/`newInstance` for `jvm-new` — the same reflective style
Phase 1 already proved out in `add_jar_url` (`jvm_thread.rs`). No code
generation, no per-class bindings; works against any already-loaded class,
including ones loaded at runtime via `jvm-load-jar`. This isn't just a
Phase 2 shortcut — Phase 3 (Clojure eval) needs fully dynamic, runtime-
discovered dispatch regardless, since eval'd code references classes with
no compile-time knowledge, so reflection-based dispatch is close to a hard
requirement for the project's trajectory. Generated typed bindings, if ever
wanted, could only be an optional ergonomic layer on top of this later.

## Value marshalling

Marshalling is **type-directed**: reflective overload resolution first
picks a candidate `Method`/`Constructor`, then each Lisp argument is
coerced to match that specific resolved parameter's declared type — not a
fixed, context-free Lisp→Java mapping. This matters because `Method.invoke`
only allows widening primitive conversions (a boxed `Long` cannot satisfy
an `int` parameter, for example), so picking the right box type per call
site is load-bearing for correctness, not cosmetic.

Default table (Lisp → Java):

| Lisp | Java |
|---|---|
| integer (fixnum or bignum, when it fits) | numeric type matching the resolved parameter (`int`/`long`/`short`/`byte`/`float`/`double`) |
| integer, any size, when the resolved parameter is `BigInteger` | `java.math.BigInteger`, via decimal-string round-trip |
| bignum too large for the resolved parameter's primitive type, parameter not `BigInteger` | marshalling error — no silent truncation |
| float | matching floating type |
| string | `java.lang.String` |
| `t` | `Boolean.TRUE` |
| `nil` | `Boolean.FALSE` if the resolved parameter is `boolean`/`Boolean`, otherwise `null` |
| a Lisp function | auto-wrapped as `LispFunction` at the marshalling boundary — this is the actual trigger point for "wrap a Lisp value as a JVM object," not a separate explicit wrap call |
| a wrapped JVM object (`user-ptr`) | unwrapped to the underlying object via its `GlobalRef` |

Return direction (Java → Lisp) is symmetric and unambiguous: `null` →
`nil`, primitives/`String`/`Boolean`/`BigInteger` → the obvious Lisp
equivalent, anything else → a new wrapped `user-ptr` handle.

**Deliberate cut**: no automatic Lisp list/vector ↔ Java `List`/array
marshalling. Genuinely ambiguous (element type inference, `List` vs. array
vs. varargs) and not needed for the interop MVP — a `java.util.ArrayList`
can still be built and populated the reflective way like any other object.
Purely additive later if it turns out to matter.

**Bignum mechanism**: decimal-string round-trip (format the Elisp bignum's
digits, `new BigInteger(String)`; reverse via `BigInteger.toString()`
parsed back into a Lisp integer), rather than copying raw sign+magnitude
limb arrays directly. Both representations are conceptually sign+magnitude-
words, but Emacs's GMP-based `emacs_limb_t` width isn't guaranteed to match
`BigInteger`'s `int[]` mag array, making direct copying a real place to
introduce an endianness/word-size bug for no benefit at prototype scale.
Revisit only if bignum-heavy interop turns out to be a real hot path.

**Crate dependency policy**: wherever the high-level `emacs` crate doesn't
expose something Phase 2 needs (bignum extraction/construction via
`extract_big_integer`/`make_big_integer`, `user-ptr` finalizers), fall back
to the lower-level `emacs-module` crate's raw/unsafe bindings directly,
rather than blocking the design on it.

## Worker thread role

Phase 1's worker thread does all JNI work for the JVM's entire lifetime.
Phase 2 changes its job description: it becomes a **dispatcher that never
blocks**, plus the sole consumer of the `Cmd` channel (a structural fact of
`std::sync::mpsc::Receiver` being single-consumer, not a design choice —
see "JVM side" under "Threading model" below).

- `Cmd::InvokeReply` is handled inline, cheaply: look up `id` in a pending-
  continuations table (`Mutex<HashMap<id, GlobalRef<CompletableFuture<Object>>>>`),
  resolve it via a JNI call on the worker thread's own (persistent)
  attachment, done.
- `Cmd::Call` is **not** run inline. Even without blocking-callback support,
  an arbitrary Java method call can take a long time for unrelated reasons
  (slow computation, I/O). Running it on the worker thread's own loop would
  stall every other pending command (other calls, `jvm-pause`, `jvm-stop`)
  behind it. Instead, each `Cmd::Call` is dispatched onto its own short-
  lived, JNI-attached thread, keeping the dispatch loop free. This mirrors
  `jvm-stop`'s existing pattern of running `JNI_DestroyJavaVM` on a
  throwaway thread so a hang doesn't block the module (see Phase 1 spec,
  "Pause vs. stop").
- `Cmd::ReleaseRef` is handled inline (a `GlobalRef` drop is cheap).

## Control plane vs. data plane: one channel, tag-differentiated

Decision: **do not** physically split control (`status`, `release`) from
data (`call-done`, `invoke`) traffic. Reuse one channel per direction, as
Phase 1 already has, distinguishing purely by message tag. Both consumers
are single-queue by construction already (Lisp's filter processes one line
at a time off one buffer; the worker's dispatch loop drains one `mpsc`
`Receiver`), so a priority-reordering/physical-separation layer would add
real complexity without a demonstrated need. Revisit only if head-of-line
blocking (a burst of `invoke` traffic delaying a `crashed` status message
behind it in the same FIFO) turns out to matter in practice.

## Threading model

### Lisp side

Because nothing in the core async protocol blocks a Lisp thread, delivery
of any message can land on whichever Lisp thread happens to be running the
process filter at that moment — there is no "correct" thread to deliver to,
matching Phase 1's existing `load-jar-done`/`status` callback dispatch
(already thread-agnostic, unchanged by Phase 2). The filter does a table
lookup on the correlation ID and `funcall`s whatever was registered, in
whatever thread's execution context it happens to run in.

This *only* becomes a real question for the optional `-sync` convenience
wrappers Phase 2 will likely offer (mirroring `jvm-load-jar-sync`), which
deliberately poll and block one specific calling Lisp thread. Investigated
empirically and via source, since this exact scenario (multiple `make-thread`
Lisp threads each independently polling the same shared process) has no
precedent in Phase 1:

- **Source reading** (`~/Programming/Tools/emacs/src/process.c`,
  `src/thread.c`): Emacs tracks a `waiting_thread` claim per file
  descriptor (`process.c:464-466`); concurrent `accept-process-output`
  callers on the same process never race for the same read — the loser
  simply times out and retries, never double-consumes. No fairness
  guarantee, but no corruption or starvation either.
- **Empirical probe** (`.probe/lisp-thread-filter-probe/`, run against GNU
  Emacs 30.1 in batch mode): confirmed the above, but also found a real gap
  the source reading missed — **a process is locked to its creating thread
  by default** (contradicting what the source comments suggested), and any
  other Lisp thread's `accept-process-output` on it hard-errors immediately
  (`"Attempt to accept output from process ... locked to thread ..."`).
  The fix is a documented, one-line escape hatch: `(set-process-thread proc
  nil)`. Once unlocked, multiple threads polling concurrently is safe:
  which thread actually executes the filter body is not predictable (the
  probe observed it flip between runs, and even saw multiple messages
  coalesce into one filter call), but every polling thread still observes
  the full, correct shared state on every iteration, because Emacs Lisp
  threads are cooperative/non-preemptive — the shared-table mutation is
  never torn.

**Action item for implementation**: `jvm--ensure-pipe-process` must call
`(set-process-thread jvm--pipe-process nil)` right after creating the pipe
process, so any `-sync` wrapper works correctly regardless of which Lisp
thread calls it.

**Debugging note carried over from the probe**: an error raised inside a
`make-thread` body is silently swallowed — no backtrace surfaces anywhere.
Any Phase 2 code that spawns Lisp threads (if any ends up being necessary)
must wrap the thread body in `condition-case` and log failures explicitly.

### JVM side

Symmetric reasoning, no blocking anywhere in the core protocol means no
thread-affinity requirement: any JVM thread that ends up executing the
native trampoline for a wrapped Lisp value (which, by definition of being
inside a native method, is already JNI-attached) can safely call
`channel::send` directly to post an `(invoke ...)` message. This is already
proven safe for concurrent Rust-side callers — Phase 1's `channel::send`
already serializes writes via `Mutex<Box<dyn Write + Send>>`, and already
has two concurrent callers today (the worker thread and `jvm-stop`'s
throwaway teardown thread).

What's new for Phase 2: a shared `Mutex<HashMap<id, ...>>` tracking pending
Java-side continuations, written by whichever JVM thread registers a new
outbound `invoke` request, and drained only by the worker thread — which is
the sole JNI-attached, persistent thread that resolves them (see "Worker
thread role" above; this is a structural consequence of `mpsc::Receiver`
being single-consumer, not a design choice made for symmetry with the Lisp
side).

Correlation IDs are generated from the existing global atomic counter
pattern (`worker::next_request_id`, currently used for `load-jar` requests)
rather than a new per-purpose counter.

## Error handling

**Transport level**: a channel write failure is silently dropped, exactly
as Phase 1 already does in `channel::send` — the worker thread has no way
to recover from a broken channel, and any `-sync` wrapper's timeout is what
catches a message that never arrives. An unrecognized message tag is logged
and ignored, matching the existing `jvm--dispatch-message` fallback clause
in `elisp/emacs-jni.el`.

**Application level**, reusing Phase 1's existing `(ok . err)` convention
rather than inventing a new shape:

- **JVM → Lisp** (a `Cmd::Call` throws): caught via JNI's exception-check/
  describe/clear, delivered as `(call-done ID nil nil "ExceptionClass:
  message")`; the `-sync` wrapper returns `(nil . ERR-STRING)`, matching
  `jvm-load-jar-sync` exactly.
- **Lisp → JVM** (a Lisp error raised while handling an `(invoke ...)`
  request): `InvokeReply` gains an ok/err case; on `err`, the worker thread
  resolves the pending future via `.completeExceptionally(new
  LispError(...))` instead of `.complete(...)` — `LispError` is the third
  class in the compiled shim, alongside `LispCallable`/`LispFunction` (see
  "Calling convention").

## Testing

- `.probe/lisp-thread-filter-probe/` (pure Elisp, `.el` + README) is
  already committed evidence for the Lisp-side threading claims above, in
  the same spirit as Phase 1's `.probe/signal-probe/`.
- New `cargo test` coverage for `protocol.rs`'s new message formatters
  (`call-done`, `invoke`, `release`), following the exact pattern of the
  existing `status_message`/`load_jar_done_message` unit tests.
- ERT integration coverage for the full round trip: `jvm-call` against a
  known fixture class/method (extending `test/fixtures/Greeter.java` or a
  sibling fixture), a Lisp function passed as a callback arg and invoked
  from Java, and both error-propagation directions. Left to the
  implementation plan to break into concrete tasks.
- The shim's own compile step (`build.rs` + `javac`) needs a build-time
  check that fails loudly if `javac` isn't on `PATH`, matching the existing
  `cargo test`/`run-ert-tests.sh` reliance on a JDK being available.

## Out of scope

Carried over from the original three-phase decomposition, plus this
document's own scope cuts:

- Wrapping a Lisp value to satisfy a pre-existing synchronous Java
  interface (`Comparator`, `Runnable`, etc.) — ruled out, see "Design
  principle" above.
- Cycles in the cross-runtime object graph — accepted limitation carried
  over from the original scope; see the concrete example under "Identity
  and lifetime."
- Automatic Lisp list/vector ↔ Java `List`/array marshalling — see "Value
  marshalling."
- Clojure evaluation, nREPL (Phase 3).
- macOS/Windows support (carried over from Phase 1).
