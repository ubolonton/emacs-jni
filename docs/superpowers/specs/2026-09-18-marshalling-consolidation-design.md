# Marshalling consolidation — design

Status: proposed. Handoff for a future session to plan (superpowers:writing-plans)
then implement. Experimental, additive-then-subtractive; keep tests green at each
step. Builds on the shipped unified symbol/keyword work
(`docs/superpowers/specs/2026-09-16-unify-symbol-marshalling-design.md`).

## Motivation

Value conversion between Lisp, the JVM, and Clojure happens at many sites across
two subsystems that grew at different times. Some of that multiplicity is
essential (a worker OS thread cannot touch Lisp); much of it is incidental
(duplicated functions, an older text protocol never retrofitted onto the newer
queue mechanism, Clojure-only machinery that could be general). The recent
symbol work exposed the cost: a new `LispArg` variant had to be threaded through
several near-duplicate places, and one twin (`lisp_value_to_arg_no_callback`)
was missed, silently breaking keyword/symbol returns from a callback.

This spec consolidates the reducible duplication and generalises the
Clojure-only behaviours into the core, leaving Clojure as a thin layer on top.

## Current state (map)

Two subsystems, two value vocabularies:

- **Generic JVM bridge** — `commands::LispArg` (`src/commands.rs:17`), used by
  `jvm-call`/`jvm-new` (async worker) and `jvm-call-direct` (main thread).
- **Clojure Lisp-access** — `LispRequestArg` (`src/lisp_requests.rs:28`, Clojure→Lisp
  args) and `commands::LispReplyValue` (`src/commands.rs:43`, Lisp→Clojure results
  incl. non-local exits), driven by the `Lisp.*` natives and `emacs-jni.clj`'s
  `->lisp`/`<-lisp`.

The Java-facing halves are already single paths: `lisp_arg_to_jvalue`
(`marshal.rs:10`) for Lisp→JVM, `object_to_lisp_arg` (`marshal.rs:346`) for
JVM→Lisp. The Lisp-facing halves are where the multiplicity lives:

| Site | Value→LispArg / LispArg→Value | Notes |
|---|---|---|
| Lisp→JVM call args | `lisp_value_to_arg` (`lib.rs:318`) | full: keyword/symbol/callback arms |
| Callback return → JVM | `lisp_value_to_arg_no_callback` (`lib.rs:272`) | **trimmed twin**, missing those arms |
| JVM→Lisp return, direct | `lisp_arg_to_value` (`lisp_requests.rs:131`) | main thread, live `Env` |
| JVM→Lisp return/args, async | `lisp_arg_to_sexp` (`marshal.rs:427`) → `jvm--resolve-sexp-result` (`jvm.el:121`) | worker has no `Env` → **text sexp** over the pipe |
| Clojure→Lisp args | `arg_to_value` (`lisp_requests.rs:107`) | `LispRequestArg`; `Scalar` arm already wraps `LispArg` |
| Lisp→Clojure result | `value_to_reply` (`lisp_requests.rs:250`) → `build_lisp_reply_object` (`jvm_thread.rs:578`) → `<-lisp` (`clj.clj:24`) | `LispReplyValue`, incl. Signal/Throw |

Already shared, no work: reentrant same-thread service lives in `src/reentry.rs`
and is used by both the generic direct path and the Clojure natives.

## Goals

- One Lisp-value→`LispArg` function (kill the twin).
- No text protocol. Every inbound (JVM→Lisp) message becomes a typed Rust queue
  entry drained by a native dispatch on the main thread; the pipe is a pure
  wakeup. All inbound values convert through the single `lisp_arg_to_value`.
- Lisp non-local exits are first-class in the core: distinct Java exception
  classes carrying the marshalled parts, so Clojure catches them by type and the
  `ex-info` translation becomes optional.
- The three value enums keep distinct roles but stop duplicating scalar variants,
  and get names that state their direction.

## Non-goals

- Fusing the three enums into one type. Their object variants are genuinely
  different (JVM-object handle vs Lisp-object `GlobalRef` pointer) and Signal/Throw
  are result-direction-only; a single type would carry context-invalid variants.
  We dedup, not fuse.
- Removing the `Lisp.*` convenience natives. Reimplementing them as
  Clojure-on-top is possible but is a performance/scope call left as an optional
  follow-up (see Item 5).
- Any change to the JVM lifecycle *state machine*, classpath, or
  reflection/overload resolution. (Item 2 changes how `status` messages are
  *delivered* to Lisp — typed queue instead of text — not the state machine or
  the `jvm-status` contract.)

## Work items

Each item is independently shippable and independently testable; the ordering in
"Sequencing" keeps the tree green throughout.

### Item 1 — one Lisp-value→`LispArg` function (Q1)

`lisp_value_to_arg_no_callback` (`lib.rs:272`) is a trimmed copy of
`lisp_value_to_arg` (`lib.rs:318`) missing the keyword, symbol, and
`functionp`→`Callback` arms. It exists only because Task 6 chose not to support a
callback returning a bare function. The omission also drops keyword/symbol
support, so a callback returning `:a` currently signals `jvm-invalid-argument`.

Change: delete `lisp_value_to_arg_no_callback`; call `lisp_value_to_arg` from
`jvm_invoke_reply` (`lib.rs:243`). Behaviour change: a callback that returns a
Lisp function now marshals it as a `Callback` (registered in
`jvm--lisp-callbacks`), which is more correct, not less.

Test: a JVM→Lisp callback returning a keyword, a symbol, and a function; assert
each round-trips (keyword/symbol as themselves, function as a callable
`LispFunction` on the JVM side).

### Item 2 — remove the text protocol entirely; native inbound dispatch (Q2)

The inbound direction (worker/JVM → Emacs main thread) is the ONLY user of the
text protocol. `channel::send` (`src/channel.rs:21`) writes formatted sexps into
a pipe opened by `env.open_channel`; the Elisp filter `jvm--process-filter`
(`jvm.el:130`) buffers partial lines, `read`s each, and `jvm--dispatch-message`
pcase-dispatches `call-done`/`invoke`/`lisp-request`/`release`/`log`/`status`.
The reverse direction already avoids text — main → worker rides a typed
`mpsc::Sender<Cmd>` (`src/worker.rs:9`).

`lisp_requests` is already the target pattern for ONE message type: it `enqueue`s
a typed Rust value and sends a bare `(lisp-request)` wakeup (`shim.rs:406`); the
native `jvm--service-lisp-requests` (`lib.rs:226`) drains and converts with a
live `Env`. Generalise that to EVERY inbound message.

Change:

- Add a typed inbound queue (generalise `lisp_requests`' queue), with a variant
  per current message: `CallDone { id, outcome }`, `Invoke { id, handle, args:
  Vec<LispArg> }`, `Release { handle }`, `Log { text }`, `Status { status,
  detail }`, plus the existing lisp-access requests. `outcome` is the structured
  value-or-exit from Item 3.
- The pipe becomes a pure wakeup: `channel::send` writes a single constant byte
  (no formatting). Producers `enqueue(msg)` then poke the pipe.
- Dispatch moves to the native side: one drain defun (extend/rename
  `jvm--service-lisp-requests` to `jvm--service-inbound`) runs on the main thread
  when the filter fires, drains ALL queued messages FIFO, and handles each with a
  live `Env` — `CallDone` resolves the pending call via `lisp_arg_to_value`,
  `Invoke` funcalls the callback and replies with `Cmd::InvokeReply`, etc. The
  filter is a trivial `(lambda (_proc _data) (jvm--service-inbound))`, or a
  `#[defun]` set directly as the filter (either works; a module function is a
  valid process filter and runs on the main thread).

Deletes: all of `src/protocol.rs`'s value/message formatters, `lisp_arg_to_sexp`
(`marshal.rs:427`) and the `(jvm-handle …)`/`(jvm-symbol …)` wire tags,
`jvm--process-filter`'s line buffering, `jvm--dispatch-message`, and
`jvm--resolve-sexp-result`. Every inbound value converts through the single
`lisp_arg_to_value`. No text protocol remains; the pipe carries only wakeups.

Why the pipe stays at all: a background thread cannot touch Lisp (Emacs is
single-threaded), so it still needs to wake the main thread's event loop. The
pipe + process filter is that mechanism; only its payload goes away. Wakeups may
coalesce — harmless, because each drain empties the whole queue (the invariant
`lisp_requests` already relies on: enqueue-then-poke; a drain sees every item
enqueued before its poke, later items ride the next poke).

Staging (keep green): (1) build the typed queue + native drain alongside the
text path, migrating one message type at a time (`call-done`, then `invoke`,
`release`, `log`, `status`); (2) once all types are migrated and the Elisp
dispatch is unused, delete the text formatters and the Elisp reader/dispatch.

Tests: every existing `jvm-call`/`-await`, callback, nREPL, and status test must
stay green through each migration step; add a keyword/symbol return over the
async path (previously exercised only via the now-removed sexp reify).

### Item 3 — first-class Lisp non-local exits in the core (Q3b)

Today a Lisp signal/throw crossing to the JVM is one neutral carrier
`LispNonLocalExit` (`kind` + `symbolOrTag` + `dataOrValue`,
`java/emacs_jni/core/LispNonLocalExit.java`), and `emacs-jni.clj` rethrows it as
`ex-info` (`lisp-exit->ex-info`/`throw-lisp-exit`, `clj.clj:44-62`;
predicates `lisp-signal?`/`lisp-throw?`). The generic async `invoke` reply does
worse — it degrades a Lisp error to a formatted string (`jvm--invoke-reply`,
`lib.rs:243` and `jvm.el` invoke case).

Change: give signals and throws their own platform-loader classes,
`LispSignal` and `LispThrow`, both extending `LispNonLocalExit` (keep the base
for a single catch point), each exposing the marshalled `symbol`/`data` or
`tag`/`value`. The core produces these for BOTH the Clojure-apply reply
(`build_lisp_reply_object`/`complete_future_with_reply`, `jvm_thread.rs:578`/`654`)
and the generic callback reply (Item 2's outcome). Then:

- Clojure code catches `LispSignal`/`LispThrow` by type and reads the fields
  directly. The `ex-info` translation and `lisp-signal?`/`lisp-throw?` become
  unnecessary — remove them, or keep a thin optional `ex-info` shim only if a
  concrete consumer wants `ex-data`. DECISION BELOW.
- The generic async callback path stops stringifying: an uncaught Lisp exit from
  a callback surfaces to the JVM as a real `LispSignal`/`LispThrow`, matching
  what `jvm-call-direct` already reconstructs on the Lisp side
  (`DirectError::NonLocalExit`, `lib.rs:195`).

Tests: Clojure `apply` of a Lisp fn that `signal`s / `throw`s is caught by type
with correct symbol/data; a generic JVM→Lisp callback that signals surfaces the
structured exception (not a string).

### Item 4 — rename and dedup the value enums (enums)

Keep three types (distinct roles) but (a) rename for direction and (b) make the
Clojure result type compose `LispArg` instead of re-listing scalars, mirroring
how `LispRequestArg::Scalar(LispArg)` already composes it (`lisp_requests.rs:120`).

Recommended names (planner may adjust):

| Now | Proposed | Role |
|---|---|---|
| `LispArg` | `BridgeValue` | value crossing the generic Lisp↔JVM bridge (scalars, symbol, JVM-object handle, Lisp-callback handle) |
| `LispRequestArg` | `LispInput` | a value handed INTO Lisp from Clojure (Lisp-object `GlobalRef` ptr, symbol, or `Scalar(BridgeValue)`) |
| `LispReplyValue` | `LispOutcome` | a result coming FROM Lisp (a value, a Lisp-object ptr, or `Signal`/`Throw`) |

Dedup: refactor `LispOutcome` to `Value(BridgeValue) | Ptr | Signal | Throw` so
its scalar/symbol cases reuse `BridgeValue` rather than duplicating `Nil`/`Int`/
`Float`/`BigInt`/`Str`/`Symbol`. This is the "reducible part" — the scalar
overlap — without fusing the direction-specific object and exit variants.

Tests: pure Rust round-trip of each enum's variants; no behaviour change, so the
full existing suite is the gate.

### Item 5 — `Lisp.*` conveniences as Clojure-on-top (Q3c, OPTIONAL)

`Lisp.intern`/`typeOf`/`eq`/`isNotNil`/`prin1ToString` are bespoke natives with
their own `LispRequest` variants; each is just a funcall of a Lisp primitive.
They could be `emacs-jni.clj` functions over the general reentrant call path,
shrinking the native surface and the `LispRequest` enum. Left OPTIONAL: the
natives avoid a full `apply` round trip per call (a real, if small, cost).
Recommend deferring until Items 1–4 land and the payoff is measured.

## Decisions

- Keep three enums (do not fuse). Rationale in Non-goals.
- The text protocol is removed entirely (Item 2). ALL inbound message types
  move to the typed queue, including `status`/`log`/`release` — not just value
  payloads — so nothing is left to justify keeping a text framing. The pipe is a
  pure wakeup.
- Non-local exits become first-class classes shared by both reply paths (Item 3).
- `ex-info` is removed by default: Clojure catches `LispSignal`/`LispThrow` by
  type. (See open question if a consumer needs `ex-data`.)

## Open questions for the planner / user

1. Enum names — confirm `BridgeValue`/`LispInput`/`LispOutcome` or pick others.
2. `ex-info` — remove entirely, or keep a one-line optional wrapper for callers
   who prefer `ex-data` over field access? Default: remove.
3. Item 2 — confirm ALL inbound types move (incl. `status`/`log`), fully
   deleting `src/protocol.rs` and the Elisp dispatch. Default: yes. Alternative:
   stop at value payloads and keep a minimal text framing for lifecycle — not
   recommended (leaves two mechanisms).
4. Item 2 filter form — a trivial Elisp filter calling one native drain defun,
   or a `#[defun]` set directly as the process filter? Default: trivial Elisp
   filter → native drain (smaller change, mirrors today's `lisp-request` path).
5. Item 5 — do it, or leave the natives as an optimisation? Default: leave.

## Sequencing and risk

Recommended order, each green before the next:

1. **Item 1** (smallest, fixes the confirmed callback keyword/symbol bug).
2. **Item 4 rename** (mechanical; do before the deeper refactors so later diffs
   use the final names). The `LispOutcome` dedup can ride with Item 3.
3. **Item 3** (exit classes; the `LispOutcome` shape settles here).
4. **Item 2** (the whole inbound path onto the typed queue + native drain,
   deleting the text protocol; staged per message type). Depends on Item 3's
   outcome type for `CallDone`/`Invoke`. Largest item — land it last of the
   non-optional ones.
5. **Item 5** (optional, last).

Risks:
- Item 2 is the largest and highest-risk: it touches async call completion
  (`jvm--pending-call` matching), the worker↔main handoff, and status/lifecycle
  delivery (`jvm-status`). Mitigate by staging — build the typed queue + native
  drain alongside the text path and migrate one message type at a time
  (`call-done` → `invoke` → `release` → `log` → `status`), keeping the suite
  green at each step; delete `src/protocol.rs` and the Elisp dispatch only once
  every type is migrated. A wakeup byte must follow every `enqueue` (the
  enqueue-then-poke invariant) or a message can sit undrained until the next poke.
- Item 2 removes the human-readable wire — text-log debugging of the channel is
  lost. Minor; add a `Log` queue variant (kept) and optionally a debug dump of
  the queue.
- Item 3 changes a public Clojure-facing contract (exception type instead of
  `ex-info`). Any external consumer catching `ex-info`/using `lisp-signal?`
  breaks — acceptable for this solo prototype, but note it in the changelog.
- Removing the sexp wire tags (Item 2) deletes `jvm--resolve-sexp-result`
  handling; confirm nothing else emits `(jvm-handle …)`/`(jvm-symbol …)`.

## Testing strategy

- Rust: `cargo test` (unit round-trips for the renamed/dedup'd enums, Item 4).
- ERT (needs `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars"`):
  `jvm-call`/`-await`/`-direct`, callback-wrap, symbol-marshal, both eval files —
  all must stay green. Add: callback returning keyword/symbol/function (Item 1);
  async keyword/symbol return (Item 2); Clojure + generic callback signal/throw
  caught by type (Item 3).
- Keep each item's commit self-contained with its own red→green tests, per the
  project's TDD rule.
