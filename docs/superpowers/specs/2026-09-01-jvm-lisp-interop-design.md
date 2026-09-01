# Phase 2 Design: Bidirectional JVM ↔ Lisp Interop

Status: draft — transport, lifecycle, and threading model settled; calling
convention (Java-side interface shape, value marshalling, error
propagation) deferred to a follow-up revision of this same document, before
this spec moves to `writing-plans`.

Date: 2026-09-01

## Purpose

Phase 2's overall scope (per `HANDOFF.md` / `README.md`'s roadmap) is four
tightly-coupled deliverables: wrap a Lisp value as a JVM object, wrap a JVM
object as a Lisp value, JVM calls a Lisp function, Lisp calls a JVM method.
All four sit on top of one shared foundation: a message-passing protocol
between the JVM worker thread(s) and the Lisp thread(s), plus the lifetime
mechanics for values that cross the runtime boundary. This document covers
that foundation. It does **not** yet cover: what Java-side interface shape
a wrapped Lisp function actually implements, how Lisp values marshal to/from
Java types, or how errors on either side surface on the other — those are
calling-convention decisions that build on top of this transport and will
be added to this document once resolved, before an implementation plan is
written.

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
the interop protocol itself. (The exact Java-side interface(s) Phase 2 does
define — necessarily async-native in shape — is calling-convention work,
deferred; this section only fixes the constraint they must satisfy.)

One consequence worth stating plainly: because nothing blocks, there is no
concept of "the thread waiting for this reply" anywhere in the core
protocol, on either side. A reply is delivered to whatever thread happens to
process it, which looks up the correlation ID in a shared table and acts —
full stop. Both threading sub-sections below follow from this.

## Wire protocol extension

Extends Phase 1's existing S-expression protocol (`src/protocol.rs`,
`src/commands.rs`) with new tags, following the same shape as the existing
`status`/`load-jar-done`/`log` messages. Field types for `ARGS`/`RESULT`
(and the equivalent `Cmd` fields) are intentionally left unspecified here —
that's exactly the marshalling question this document defers.

Worker → Lisp (`channel::send`, new tags alongside `status`/`load-jar-done`/`log`):

- `(call-done ID OK RESULT ERR)` — result of a Lisp-initiated JVM call
  (`Cmd::Call`). Same OK/ERR shape as `load-jar-done`.
- `(invoke ID HANDLE ARGS)` — the JVM, via a wrapped-Lisp-value proxy object,
  asking Lisp to run the Lisp value identified by `HANDLE`.
- `(release HANDLE)` — the JVM-side proxy object for `HANDLE` was garbage
  collected; Lisp may drop its corresponding entry.

Lisp → worker (new `Cmd` variants, same `mpsc` channel as `Cmd::Pause`/`Cmd::Stop`/`Cmd::LoadJar`):

- `Cmd::Call { id, target, member, args }` — Lisp asks the JVM to invoke a
  method (or construct an object; exact shape TBD with calling convention).
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
  collects it, so Rust can drop the `GlobalRef`. **Dependency to confirm**:
  this assumes the `emacs` crate (emacs-module-rs) supports attaching a
  finalizer to a `user-ptr` value — needs a quick check against the crate
  source before this becomes load-bearing.

This reuses Phase 1's existing "no cycles in the cross-runtime object
graph" assumption (stated in the original three-phase decomposition) —
independent per-side reference tracking is exactly what breaks down if a
Lisp value and a JVM object end up holding live references to each other.
Worth re-confirming this assumption still holds once the calling convention
makes it concrete what kinds of values actually get wrapped.

## Worker thread role

Phase 1's worker thread does all JNI work for the JVM's entire lifetime.
Phase 2 changes its job description: it becomes a **dispatcher that never
blocks**, plus the sole consumer of the `Cmd` channel (a structural fact of
`std::sync::mpsc::Receiver` being single-consumer, not a design choice —
see "JVM-side threading" below).

- `Cmd::InvokeReply` is handled inline, cheaply: look up `id` in a pending-
  continuations table (`Mutex<HashMap<id, GlobalRef<CompletableFuture-or-
  equivalent>>>`), resolve it via a JNI call on the worker thread's own
  (persistent) attachment, done.
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

## Error handling (transport level only)

Settled, at the transport level: a channel write failure is silently
dropped, exactly as Phase 1 already does in `channel::send` — the worker
thread has no way to recover from a broken channel, and any `-sync`
wrapper's timeout is what catches a message that never arrives. An
unrecognized message tag is logged and ignored, matching the existing
`jvm--dispatch-message` fallback clause in `elisp/emacs-jni.el`.

Deferred: how a Java exception thrown during a `Cmd::Call` surfaces as a
Lisp-visible error (likely folds into the existing `(call-done ID OK RESULT
ERR)` OK/ERR shape, same pattern as `load-jar-done` — but the exact
exception-to-string/structured-error mapping is calling-convention work),
and how a Lisp error raised while handling an `(invoke ...)` request
propagates back to the JVM side (an `InvokeReply` variant carrying an error
case, or a separate tag — undecided).

## Testing

- `.probe/lisp-thread-filter-probe/` (pure Elisp, `.el` + README) is
  already committed evidence for the Lisp-side threading claims above, in
  the same spirit as Phase 1's `.probe/signal-probe/`.
- New `cargo test` coverage for `protocol.rs`'s new message formatters
  (`call-done`, `invoke`, `release`), following the exact pattern of the
  existing `status_message`/`load_jar_done_message` unit tests.
- ERT integration coverage for the full round trip is deferred until the
  calling convention is defined (there's no concrete Lisp-callable API
  surface to test yet).

## Open questions, deferred to a follow-up revision of this document

- Calling convention: what Java-side interface(s) does Phase 2 define for
  a wrapped Lisp value, and what does `jvm-call`'s "target" argument (which
  JVM object/class to invoke a method on) actually look like from Lisp?
- Value marshalling rules between Lisp values and Java types.
- Application-level error propagation semantics (see "Error handling"
  above).
- Whether the `emacs` crate's `user-ptr` support includes finalizers (see
  "Identity and lifetime" above) — needs a quick check against crate
  source, not yet done.

## Out of scope

Carried over from the original three-phase decomposition, plus this
document's own scope cut:

- Wrapping a Lisp value to satisfy a pre-existing synchronous Java
  interface (`Comparator`, `Runnable`, etc.) — ruled out, see "Design
  principle" above.
- Cycles in the cross-runtime object graph (assumption carried over from
  the original scope; worth re-confirming once the calling convention is
  concrete).
- Clojure evaluation, nREPL (Phase 3).
- macOS/Windows support (carried over from Phase 1).
