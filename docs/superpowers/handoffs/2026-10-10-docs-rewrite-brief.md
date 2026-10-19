# Brief for the user-docs rewrite (shared by all subagents)

Repo: /home/ubolonton/Programming/projects/emacs-jni. Read the review first:
`docs/superpowers/handoffs/2026-10-10-user-docs-review-handoff.md`.

## Rules for every agent

- Edit ONLY the files your task assigns. Other agents edit other docs in parallel.
- Do NOT commit, do NOT run `cargo build`/`cargo test`/`test/run-ert-tests.sh`. The host is a
  shared, resource-constrained VM. The orchestrator verifies and commits.
- Verify every fact you write against the code (`src/`, `elisp/`, `clojure-src/`, `java/`,
  `test/`). Do not trust the old docs. Do not trust `docs/superpowers/` (historical, often stale).
- Never invent behavior. If unsure, read the code; if still unsure, leave the claim out and
  report it.
- Keep cross-doc links valid. Heading anchors: GitHub style (lowercase, spaces → `-`, punctuation
  dropped). If you rename a heading that other docs link to, report the old → new anchor.
- Final report: files changed, facts you could not verify, anchors renamed, anything left for
  another doc. Keep it short.

## Style

- Diátaxis (https://diataxis.fr/): tutorial = learning by doing, "we", a guaranteed working
  path, no choices or explanations; how-to = task-titled ("How to X"), steps for a competent
  user, no teaching or rationale; reference = austere, complete, accurate facts in tables, no
  rationale; explanation = why/how it fits, for understanding, no steps.
- ASD-STE100, STE-flavored mode: active voice, simple present, ≤25 words per sentence
  (≤20 for steps), one idea per sentence, no semicolons, few em-dashes, no phrasal verbs,
  no nominalizations, no marketing adjectives.
- No history ("was discovered", "found empirically", "originally", "now"), no defensive asides
  ("not an oversight", "deliberately", "not a choice this project made"), no intensifiers
  (real, really, exactly, actually, genuinely, just, simply, its own).
- Hard-wrap prose at ~100 columns. Senior-engineer audience. Fewer words is better, but never
  drop a condition, a hedge, or a number.
- Single source per fact: limitations live ONLY in `docs/explanation/limitations-and-tradeoffs.md`
  (link to it). Rust internals (module paths, type names, crate versions) live ONLY in the new
  contributor page `docs/explanation/implementation.md`. User docs may name Lisp/Clojure/Java
  API symbols, never Rust paths.

## Glossary (use these terms only)

| Term | Meaning | Do not say |
|---|---|---|
| worker thread | The Rust thread that creates the JVM and runs async commands | JVM thread (ambiguous) |
| main thread | The Emacs main thread | |
| direct path / `-direct` | Runs on the main thread, synchronously | sync, `-sync` |
| `-await` | Runs on the worker, the caller blocks with a timeout | `-sync` |
| `-async` | Runs on the worker, result goes to a callback | |
| wrapped JVM object | A JVM object held by Lisp (a `user-ptr`) | handle, token, jvm-handle, opaque handle |
| `LispObject` | An opaque Lisp value held by Java/Clojure | handle |
| `LispSymbol` | Java class carrying a Lisp symbol/keyword name | marker (ok once, when defined) |
| `LispFunction` | Java object wrapping a Lisp function | proxy (ok once) |

## Current facts (verified 2026-10-10)

- Bare names: `jvm-call`/`jvm-new`/`jvm-load-jar` and all `jvm-clj-*` alias `-direct`.
  Lifecycle `jvm-start`/`jvm-pause`/`jvm-stop` alias `-await` (no `-direct`).
- `-await` takes an optional trailing TIMEOUT (default 30 s), signals `jvm-timeout`. `-async`
  and `-direct` take no timeout.
- Direct path: the main thread attaches to the JVM once and stays attached, so
  `Thread.currentThread()` and thread-locals (`*ns*`, dynamic bindings) persist across direct
  calls. Other Lisp threads attach per call. A direct call blocks Emacs (holds the global lock).
- Async path: Lisp sends a command to the worker thread over a channel. `jvm-call-async`
  spawns one OS thread per call (so thread-locals do not persist across `-await` evals; a panic
  in that thread is not caught and the caller times out). Results return through a typed queue;
  a pipe process carries a one-byte wakeup; its filter `jvm--service-inbound` drains the queue
  on the main thread.
- Clojure → Lisp (`emacs-jni.clj` `apply`, `intern`, `type-of`, `eq`, `is-not-nil?`,
  `prin1ToString`): Lisp values need the main thread. If the Clojure code runs inside a
  `jvm-call-direct` on the main thread, the request runs immediately on that thread (no queue,
  no deadlock). Otherwise (worker threads, nREPL threads) it is queued, Emacs runs it when it
  next drains the queue (idle, or inside an `-await` wait loop), and the JVM thread blocks with
  a 30 s timeout.
- A Lisp signal/throw raised in a callback reaches Java as `LispSignal`/`LispThrow` (subclasses
  of `LispNonLocalExit`). If it unwinds back out of a `jvm-call-direct`, Lisp gets the original
  signal/throw again. Other Java/Clojure exceptions → `jvm-error`.
- Marshalling Lisp → Java, type-directed by the declared parameter type:
  `nil`→`false` (for `boolean`/`Boolean`) else `null`; `t`→`true`; integer → `int`/`short`/`byte`
  (range-checked), `long`/`Object` (Long), `float`, `double`, `BigInteger`; float → `float`,
  `double`, `Object` (Double); bignum → `BigInteger`/`Object`; string → `String`; wrapped JVM
  object → itself; **keyword or symbol (any symbol, even one with a function definition) →
  `LispSymbol`**; a non-symbol function (lambda, closure, byte-code, subr) → `LispFunction`;
  anything else (vector, hash table, …) signals `jvm-invalid-argument`.
- Java → Lisp: void/null → `nil`; boolean → `t`/`nil`; integral (boxed or not) and `char` →
  integer; float/double → float; `String` → string; `BigInteger` → integer (bignum if large);
  `LispSymbol` → interned symbol/keyword; anything else → wrapped JVM object.
- Clojure eval (`jvm-clj-eval-string*`) calls `emacs-jni.clj/eval-for-lisp`, so
  `emacs-jni.clj` must be loaded first: use `jvm-clj-start` (loads `jvm-clj-classpath` +
  `clojure-src/`, requires the ns). `jvm-clj-nrepl-start` calls it too. Keyword/symbol results
  return as Lisp keywords/symbols; vectors/maps return as wrapped JVM objects.
- Overload resolution: candidates with matching name and arity, in `getMethods()` order; skip a
  candidate if marshalling fails or `invoke` throws a bare `IllegalArgumentException`. First
  success wins, no specificity. Real ambiguity is numeric: `(jvm-call "java.lang.String" "valueOf"
  '(42))` → `"42.0"` (a float/double overload won). `long` vs `String` with a number is
  deterministic (the `String` candidate fails marshalling).
- `jvm-error` messages currently start with "Parse failed for input:" (a code quirk, reported
  separately). Do not document the prefix as intended.
- `mise.toml` and `mise.lock` are tracked in git.
- Rust layout (for `implementation.md` only): `src/protocol.rs` (`BridgeValue`, `LispInput`,
  `LispOutcome`, `Status`, `CallTarget`); `src/lisp/{entry,convert,inbox,runtime}.rs`;
  `src/jvm/{entry,convert,inbox,invoke,runtime}.rs`. Converters: `lisp::convert::to_bridge`
  (Lisp→BridgeValue), `jvm::convert::from_bridge` (BridgeValue→JNI, type-directed),
  `jvm::convert::to_bridge` (JNI→BridgeValue), `lisp::convert::from_bridge` (BridgeValue→Lisp).
  JVM objects: `JvmObject` (`Arc<Global>`) inside a `JvmHandle` user-ptr; its `Drop` sends
  `Cmd::ReleaseRef` to the worker. No handle tables on either side. `LispObject.ptr` is a boxed
  `emacs::GlobalRef`, freed once by a `Cleaner` → `Inbound::FreeRef`. Direct-path reentrancy:
  `lisp::runtime` env stack. jni 0.22.4 caches the `JavaVM` in a process-global singleton.
