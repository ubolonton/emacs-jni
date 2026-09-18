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
- One `LispArg`→Lisp-value function for results, on both call paths — deliver the
  `LispArg` to the main thread instead of serialising it to text.
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
- Any change to the JVM lifecycle, classpath, or reflection/overload resolution.

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

### Item 2 — deliver results over the wakeup+queue, not text (Q2)

The async worker serialises a result `LispArg` to a text sexp
(`protocol::call_done_message`, `src/protocol.rs:20`) and ships it over the pipe;
the main thread's reader plus `jvm--resolve-sexp-result` (`jvm.el:121`) rebuilds
it. This predates the `lisp_requests` mechanism, which already does the better
thing: `enqueue` a Rust-side value and send a bare `(lisp-request)` wakeup
(`shim.rs:406`), then `jvm--service-lisp-requests` drains and converts with a
live `Env`.

Change: route `call-done` results (and the `invoke` callback-args path,
`protocol::invoke_message`, `src/protocol.rs:32`) through a Rust-side results
queue carrying the `LispArg` (plus id / ok / outcome), waking the main thread
with a notification; the main thread pops and runs `lisp_arg_to_value`
(`lisp_requests.rs:131`). Both call paths then converge on that one converter.

Removes: `lisp_arg_to_sexp` and the `(jvm-handle …)`/`(jvm-symbol …)` wire tags
and their reify arms in `jvm--resolve-sexp-result`. The text protocol keeps only
control/log/lifecycle framing, not value payloads.

Interaction with Item 3: the result queue entry should carry a structured
*outcome* (value or non-local exit), so design its type with Item 3 in mind (a
callback that signals delivers a Signal/Throw outcome, not a stringified error).

Tests: existing `jvm-call`/`jvm-call-await` return-value and callback tests must
stay green; add a keyword/symbol return over the async path (previously covered
only by the now-removed sexp reify).

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
- Result delivery moves off the text protocol onto the queue+wakeup mechanism
  (Item 2); the text protocol carries only control/log/lifecycle framing after.
- Non-local exits become first-class classes shared by both reply paths (Item 3).
- `ex-info` is removed by default: Clojure catches `LispSignal`/`LispThrow` by
  type. (See open question if a consumer needs `ex-data`.)

## Open questions for the planner / user

1. Enum names — confirm `BridgeValue`/`LispInput`/`LispOutcome` or pick others.
2. `ex-info` — remove entirely, or keep a one-line optional wrapper for callers
   who prefer `ex-data` over field access? Default: remove.
3. Item 2 scope — do the `invoke` (callback-args) path in the same change as
   `call-done`, or land `call-done` first and `invoke` second? Both end on
   `lisp_arg_to_value`; splitting is lower-risk.
4. Item 5 — do it, or leave the natives as an optimisation? Default: leave.

## Sequencing and risk

Recommended order, each green before the next:

1. **Item 1** (smallest, fixes the confirmed callback keyword/symbol bug).
2. **Item 4 rename** (mechanical; do before the deeper refactors so later diffs
   use the final names). The `LispOutcome` dedup can ride with Item 3.
3. **Item 3** (exit classes; the `LispOutcome` shape settles here).
4. **Item 2** (result delivery onto the queue; depends on Item 3's outcome type).
5. **Item 5** (optional, last).

Risks:
- Item 2 touches async call completion (`jvm--pending-call` matching) and the
  worker↔main handoff — the highest-risk item. Keep the text protocol's control
  messages; only value payloads move. Land `call-done` and `invoke` separately.
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
