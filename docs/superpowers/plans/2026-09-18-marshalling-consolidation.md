# Marshalling Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Collapse the reducible duplication in Lisp↔JVM↔Clojure value conversion: one Value→arg function, one inbound transport (typed queue, no text protocol), first-class Lisp-exit Java classes, and direction-named enums that stop duplicating scalars.

**Architecture:** Three value enums stay distinct by role but get direction-stating names and share a single scalar carrier. Every inbound (JVM→Emacs) message becomes a typed Rust queue entry drained by a native `#[defun]` process filter on the main thread; the pipe carries only wakeups. Lisp non-local exits cross as `LispSignal`/`LispThrow` Java classes read by field, not `ex-info`.

**Tech Stack:** Rust (jni-rs 0.22.4, emacs 0.21 crate), Emacs Lisp (module + ERT), Java 26 (platform-loader shim classes), Clojure (`emacs-jni.clj`).

**Spec:** `docs/superpowers/specs/2026-09-18-marshalling-consolidation-design.md`

## Global Constraints

- **Enum names (settled):** `LispArg→BridgeValue`, `LispRequestArg→LispInput`, `LispReplyValue→LispOutcome`.
- **ex-info (settled):** remove entirely. Clojure catches `LispSignal`/`LispThrow` by type and reads fields. Drop `lisp-exit->ex-info`, `lisp-signal?`, `lisp-throw?`.
- **Text protocol (settled):** delete fully. All inbound message types (`status`, `log`, `load-jar-done`, `call-done`, `invoke`, `release`, lisp-access) move to the typed queue. `src/protocol.rs`, `jvm--process-filter`, `jvm--dispatch-message`, `jvm--resolve-sexp-result`, and the `(jvm-handle …)`/`(jvm-symbol …)` wire tags all go away.
- **Filter form (settled):** the final inbound drain is a `#[defun]` set directly as the pipe process filter (runs on the Emacs main thread).
- **Item 5 (settled):** DEFERRED. Do not touch the `Lisp.*` convenience natives in this plan.
- **Do NOT run bare `cargo fmt`** — the installed rustfmt disagrees with committed formatting (~62 unrelated diffs). Format new lines by hand. Run `cargo clippy` on touched files only.
- Pre-existing clippy warnings live at `src/jvm_thread.rs:284,910` (untouched) — not caused by this work.
- Build/test Rust: `cargo test` (currently 42/42 unit; never use `--release`).
- ERT: `./test/run-ert-tests.sh test/<file>.el`. Clojure ERT files need `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars"` prefixed (jars already fetched; `test/fetch-clojure-jars.sh` refetches).
- Prefer integration (ERT) tests over unit tests; prefer running specific tests. `if let`/let-chains over nested; no `unwrap`/`panic!`/`unsafe` without a `SAFETY:` comment; `#[expect]` over `#[allow]`. Top-level imports. Full variable names.
- Commit prose: Simplified Technical English, minimal words. Each task is one self-contained commit with its own red→green tests.
- The untracked handoff doc `docs/superpowers/handoffs/2026-09-18-marshalling-consolidation-handoff.md` must NOT be committed. `git add` only the exact files each step names.

---

## File Structure

Files touched, by responsibility:

- `src/commands.rs` — the two generic-bridge enums: `BridgeValue` (was `LispArg`), `LispOutcome` (was `LispReplyValue`), and `Cmd`. Item 4 renames + dedups here.
- `src/lisp_requests.rs` — `LispInput` (was `LispRequestArg`), the inbound queue, the `Env`-op runners (`arg_to_value`, `lisp_arg_to_value`→`bridge_value_to_value`, `value_to_reply`→`value_to_outcome`, `run_request_value`, `service`→`service_inbound`, `compute_reply`). Grows the typed inbound queue in Item 2.
- `src/lib.rs` — the `#[defun]`s. Item 1 merges the twin here; Item 2 renames/repurposes `jvm--service-lisp-requests`→`jvm--service-inbound` and repoints the invoke reply.
- `src/marshal.rs` — Java-facing halves (`lisp_arg_to_jvalue`, `jvm_value_to_lisp_arg`/`object_to_lisp_arg`). Item 2 deletes `lisp_arg_to_sexp` + `quote_lisp_string`.
- `src/jvm_thread.rs` — worker reply builders (`build_lisp_reply_object`, `complete_future_with_reply`, `build_non_local_exit`, `throw_non_local_exit`, `handle_invoke_reply`, `handle_lisp_reply`, `handle_call`, `handle_load_jar`, status sends). Item 3 splits the exit class; Item 2 rewrites the message producers to enqueue.
- `src/shim.rs` — the JVM-thread natives that post inbound messages (`lisp_function_call`, `release_notifier_run`, `service_lisp_request_sync`). Item 2 repoints `invoke`/`release` producers.
- `src/channel.rs` — `send`. Item 2 adds a pure-wakeup `poke()`.
- `src/protocol.rs` — DELETED entirely in Item 2's final task.
- `src/worker.rs` — untouched except possibly the `Cmd` doc comment.
- `elisp/jvm.el` — filter, dispatch, pending alists, `jvm--resolve-sexp-result`, `jvm--service-lisp-requests` call site. Item 2 rewrites the filter to the native drain and deletes text dispatch.
- `java/emacs_jni/core/LispSignal.java`, `LispThrow.java` — NEW (Item 3). `LispNonLocalExit.java` stays as the shared base.
- `clojure-src/emacs_jni/clj.clj` — Item 3 replaces `ex-info` translation with typed `catch`.
- Java shim loading: `src/jvm_thread.rs` `define_classes`/`shim.rs` embeds bytecode for shim classes — Item 3 adds the two new classes to whatever `include_bytes!`/embed list exists (verify at implementation time; see Task 4 Step 1).

---

## Task 1: Merge the two `Value→BridgeValue` twins (Item 1)

Fixes the confirmed bug: a JVM→Lisp callback returning `:a` (a keyword) or a symbol currently signals `jvm-invalid-argument`, because `jvm_invoke_reply` uses the trimmed `lisp_value_to_arg_no_callback`, which lacks the keyword/symbol/`functionp` arms.

**Files:**
- Modify: `src/lib.rs:243-259` (`jvm_invoke_reply`), delete `src/lib.rs:261-299` (`lisp_value_to_arg_no_callback`).
- Test: `test/jvm-invoke-test.el` (add cases; mirrors existing `callWithArg` style).

**Interfaces:**
- Consumes: `lisp_value_to_arg(v: Value) -> Result<commands::LispArg>` (`src/lib.rs:318`), unchanged.
- Produces: no signature change to `jvm_invoke_reply`.

- [ ] **Step 1: Write the failing ERT test**

Add to `test/jvm-invoke-test.el`, before the `provide`:

```elisp
(ert-deftest emacs-jni-jvm-invoke-test/callback-returning-keyword-symbol-function ()
  "A JVM->Lisp callback may return a keyword, a symbol, or a function.
Before Item 1 the invoke-reply path used a trimmed converter missing those
arms, so a keyword return signalled `jvm-invalid-argument' and surfaced as a
`jvm-error'."
  (should (eq (jvm-start-await) 'ready))
  (jvm-load-jar-await jvm-invoke-test--calc-jar)
  ;; Keyword and symbol round-trip as themselves.
  (should (eq (jvm-call-await "Calc" "callWithArg" (list (lambda (_n) :a) 1)) :a))
  (should (eq (jvm-call-await "Calc" "callWithArg" (list (lambda (_n) 'foo) 1)) 'foo))
  ;; A function return marshals as a Callback -> LispFunction on the JVM,
  ;; which crosses back as an opaque handle (a user-ptr), not an error.
  (should (user-ptrp (jvm-call-await "Calc" "callWithArg"
                                     (list (lambda (_n) (lambda () 1)) 1)))))
```

- [ ] **Step 2: Run it to verify it fails**

Run: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-invoke-test.el`
Expected: the new test FAILs — the keyword case signals `jvm-error` (message contains `jvm-invalid-argument`).

- [ ] **Step 3: Point the invoke reply at the full converter**

In `src/lib.rs`, in `jvm_invoke_reply` (line ~251), change:

```rust
    let arg = if ok {
        lisp_value_to_arg_no_callback(result)?
    } else {
```

to:

```rust
    let arg = if ok {
        lisp_value_to_arg(result)?
    } else {
```

Then delete the whole `lisp_value_to_arg_no_callback` function (its doc comment `src/lib.rs:261-271` and body `272-299`).

- [ ] **Step 4: Run the test to verify it passes**

Run: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-invoke-test.el`
Expected: PASS (all cases, including the two pre-existing invoke tests).
Also run `cargo test` — expected: unchanged 42/42.
Run `cargo clippy` — expected: no NEW warnings (the 2 known `jvm_thread.rs` ones may remain).

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs test/jvm-invoke-test.el
git commit -m "fix: marshal callback keyword/symbol/function returns

Merge lisp_value_to_arg_no_callback into lisp_value_to_arg. A callback
returning :a signalled jvm-invalid-argument; it now round-trips."
```

---

## Task 2: Rename the three value enums (Item 4, rename only)

Pure mechanical rename, no behaviour change. Do it before the deeper refactors so later diffs use the final names. `LispArg→BridgeValue`, `LispRequestArg→LispInput`, `LispReplyValue→LispOutcome`. The scalar dedup is Task 3.

**Files (every reference across the crate):**
- Modify: `src/commands.rs` (`LispArg`→`BridgeValue`, `LispReplyValue`→`LispOutcome`, `Cmd` field types + doc comments).
- Modify: `src/lisp_requests.rs` (`LispRequestArg`→`LispInput`; update `use` and every reference).
- Modify: `src/lib.rs`, `src/marshal.rs`, `src/jvm_thread.rs`, `src/shim.rs` (every `commands::LispArg`/`LispReplyValue`, `lisp_requests::LispRequestArg`).
- Test: none new — the full existing suite is the gate.

**Interfaces:**
- Produces: `commands::BridgeValue`, `commands::LispOutcome`, `lisp_requests::LispInput` — same variants, new names. All later tasks use these names.

- [ ] **Step 1: Verify the current build is green (baseline)**

Run: `cargo test`
Expected: 42/42 pass. (Establishes the pre-rename baseline.)

- [ ] **Step 2: Rename `LispArg`→`BridgeValue` crate-wide**

Rename the enum in `src/commands.rs:17` and every occurrence of `LispArg` (as `commands::LispArg`, `LispArg::`, and bare `LispArg` in `lisp_requests.rs`/`marshal.rs`) to `BridgeValue`. Use a mechanical find/replace of the identifier `LispArg` → `BridgeValue`, then read each hunk to confirm no unrelated substring matched (there is no other identifier containing `LispArg`). Update the `use crate::commands::{... LispArg ...}` imports in `src/marshal.rs:3` and `src/lisp_requests.rs:20`.

- [ ] **Step 3: Rename `LispReplyValue`→`LispOutcome` and `LispRequestArg`→`LispInput` crate-wide**

Same mechanical rename for both identifiers across `src/commands.rs`, `src/lisp_requests.rs`, `src/lib.rs`, `src/jvm_thread.rs`, `src/shim.rs`. Update the `use crate::commands::{Cmd, LispArg, LispReplyValue}` line in `src/lisp_requests.rs:20` to `{Cmd, BridgeValue, LispOutcome}`.

- [ ] **Step 4: Build, test, clippy**

Run: `cargo test`
Expected: 42/42 pass (behaviour identical; only names changed).
Run: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-call-test.el` and `test/jvm-clj-test.el`
Expected: green (no wire/behaviour change).
Run `cargo clippy` — no new warnings.

- [ ] **Step 5: Commit**

```bash
git add src/commands.rs src/lisp_requests.rs src/lib.rs src/marshal.rs src/jvm_thread.rs src/shim.rs
git commit -m "refactor: rename value enums for direction

LispArg->BridgeValue, LispRequestArg->LispInput, LispReplyValue->LispOutcome.
No behaviour change."
```

---

## Task 3: Dedup `LispOutcome` scalars onto `BridgeValue` (Item 4, dedup)

`LispOutcome` re-lists `Nil`/`Int`/`Float`/`BigInt`/`Str`/`Symbol`, which `BridgeValue` already has. Collapse those to `Value(BridgeValue)`, mirroring how `LispInput::Scalar(BridgeValue)` already composes it. Keep `Bool`, `Ptr`, `Signal`, `Throw` as outcome-specific (a computed boolean from `eq`/`is-not-nil?` must stay a real Java `Boolean`, distinct from `nil`→null; `Ptr`/`Signal`/`Throw` have no `BridgeValue` counterpart). No behaviour change — the full suite is the gate.

**Files:**
- Modify: `src/commands.rs` (`LispOutcome` definition).
- Modify: `src/lisp_requests.rs` (`value_to_outcome`, `error_to_reply`, `free_reply_ptr`, `fallback_signal`, the `#[cfg(test)]` `fallback_signal_is_error_symbol_with_message`).
- Modify: `src/jvm_thread.rs` (`build_lisp_reply_object`, `handle_invoke_reply`'s fabricated signal).
- Test: reuse existing suite; adjust the one Rust unit test that matches `LispOutcome::Symbol`/`Str` directly.

**Interfaces:**
- Produces: `LispOutcome::Value(BridgeValue) | Bool(bool) | Ptr(i64) | Signal { symbol: Box<LispOutcome>, data: Box<LispOutcome> } | Throw { tag: Box<LispOutcome>, value: Box<LispOutcome> }`.
- Produces: `jvm_thread::build_lisp_reply_object(env, LispOutcome) -> Result<JObject>` — the `Value` arm delegates to `marshal::lisp_arg_to_jvalue(env, &bv, &object_class)` + `marshal::box_if_primitive`.

- [ ] **Step 1: Rewrite the enum**

In `src/commands.rs`, replace the `LispOutcome` variants `Ptr`,`Nil`,`Bool`,`Int`,`Float`,`BigInt`,`Str`,`Symbol`,`Signal`,`Throw` with:

```rust
#[derive(Debug)]
pub enum LispOutcome {
    /// A scalar or symbol result, reusing the generic bridge carrier — the
    /// dedup'd overlap with `BridgeValue` (was `Nil`/`Int`/`Float`/`BigInt`/
    /// `Str`/`Symbol`). `nil` is `BridgeValue::Nil`, the symbol `t` stays
    /// `Bool(true)` below (a Lisp predicate result usable as a Clojure boolean).
    Value(BridgeValue),
    /// A computed boolean from `eq`/`is-not-nil?`, or the Lisp symbol `t` —
    /// always a real Java `Boolean`, never `nil`/null.
    Bool(bool),
    /// A boxed `emacs::GlobalRef` pointer — an opaque Lisp `LispObject`.
    Ptr(i64),
    /// A Lisp `signal`: completes exceptionally with a `LispSignal`.
    Signal { symbol: Box<LispOutcome>, data: Box<LispOutcome> },
    /// A Lisp `throw`: completes exceptionally with a `LispThrow`.
    Throw { tag: Box<LispOutcome>, value: Box<LispOutcome> },
}
```

- [ ] **Step 2: Build to surface every match site (expected failure)**

Run: `cargo build`
Expected: FAIL — non-exhaustive/undefined-variant errors in `lisp_requests.rs` (`value_to_outcome`, `fallback_signal`) and `jvm_thread.rs` (`build_lisp_reply_object`). This lists exactly the sites to update.

- [ ] **Step 3: Update `value_to_outcome` (was `value_to_reply`) in `src/lisp_requests.rs`**

Rewrite its returns to wrap scalars: `Ok(LispOutcome::Nil)`→`Ok(LispOutcome::Value(BridgeValue::Nil))`; the `t` case stays `Ok(LispOutcome::Bool(true))`; `Str`→`Ok(LispOutcome::Value(BridgeValue::Str(...)))`; `Float`→`Value(BridgeValue::Float(...))`; fixnum→`Value(BridgeValue::Int(...))`; bignum→`Value(BridgeValue::BigInt(digits))`; keyword→`Value(BridgeValue::Symbol { name, keyword: true })`; interned symbol→`Value(BridgeValue::Symbol { name, keyword: false })`; fallthrough `Ptr(box_value(value))` unchanged. In `run_request_value`, the `IsNotNil`/`Eq` arms keep returning `LispOutcome::Bool(...)`.

Update `fallback_signal` to build its nested parts with the new shapes:

```rust
fn fallback_signal(message: &str) -> LispOutcome {
    LispOutcome::Signal {
        symbol: Box::new(LispOutcome::Value(BridgeValue::Symbol {
            name: "error".to_string(),
            keyword: false,
        })),
        data: Box::new(LispOutcome::Value(BridgeValue::Str(message.to_string()))),
    }
}
```

Update `free_reply_ptr`'s `if let LispReplyValue::Ptr(ptr) = *b` to `LispOutcome::Ptr(ptr)` (name only; `Ptr` still exists).

- [ ] **Step 4: Update `build_lisp_reply_object` in `src/jvm_thread.rs` to delegate the `Value` arm**

Replace the per-scalar arms with a single `Value` arm that reuses the existing Lisp→JVM marshaller (so a value crossing Lisp→JVM as a result uses the same code path as a call argument), keeping `Bool`/`Ptr`, and the `Signal`/`Throw` `unreachable!`:

```rust
pub(crate) fn build_lisp_reply_object<'local>(
    env: &mut jni::Env<'local>,
    value: crate::commands::LispOutcome,
) -> jni::errors::Result<jni::objects::JObject<'local>> {
    use crate::commands::LispOutcome;
    match value {
        LispOutcome::Value(bridge) => {
            // Reuse the one Lisp->JVM marshaller against java.lang.Object,
            // then box any primitive — identical to how handle_invoke_reply
            // completes a callback's return value.
            let object_class = env.find_class(jni::jni_str!("java/lang/Object"))?;
            let jvalue = crate::marshal::lisp_arg_to_jvalue(env, &bridge, &object_class)?;
            crate::marshal::box_if_primitive(env, jvalue)
        }
        LispOutcome::Bool(b) => {
            let class = env.find_class(jni::jni_str!("java/lang/Boolean"))?;
            env.call_static_method(
                &class,
                jni::jni_str!("valueOf"),
                jni::jni_sig!((v: boolean) -> java.lang.Boolean),
                &[jni::objects::JValue::Bool(b)],
            )?
            .l()
        }
        LispOutcome::Ptr(ptr) => {
            let class = env.find_class(jni::jni_str!("emacs_jni/core/LispObject"))?;
            env.new_object(&class, jni::jni_sig!((ptr: long) -> void), &[jni::objects::JValue::Long(ptr)])
        }
        LispOutcome::Signal { .. } | LispOutcome::Throw { .. } => {
            unreachable!("Signal/Throw are handled by complete_future_with_reply / throw_non_local_exit before build_lisp_reply_object")
        }
    }
}
```

Note: `lisp_arg_to_jvalue` against `"java.lang.Object"` already maps `Nil→null`, `True→Boolean.TRUE`, `Int→Long`, `Float→Double`, `BigInt→BigInteger`, `Str→String`, `Symbol→LispSymbol` (verified `src/marshal.rs:18-119`) — matching the old per-arm behaviour. `Object`/`Callback` arms are unreachable for an outcome (never produced by `value_to_outcome`) but are total, so no panic risk.

Update `handle_invoke_reply`'s fabricated error signal (`src/jvm_thread.rs:521-528`) to the new nested shape (`LispOutcome::Value(BridgeValue::Symbol { name: "error", keyword: false })` and `LispOutcome::Value(BridgeValue::Str(message))`).

- [ ] **Step 5: Fix the Rust unit test**

In `src/lisp_requests.rs`'s `fallback_signal_is_error_symbol_with_message`, update the matches to the wrapped shapes:

```rust
    match fallback_signal("boom") {
        LispOutcome::Signal { symbol, data } => {
            assert!(matches!(*symbol, LispOutcome::Value(BridgeValue::Symbol { ref name, keyword: false }) if name == "error"));
            assert!(matches!(*data, LispOutcome::Value(BridgeValue::Str(ref s)) if s == "boom"));
        }
        _ => panic!("expected Signal"),
    }
```

- [ ] **Step 6: Build, test, clippy**

Run: `cargo test`
Expected: 42/42 pass.
Run: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-clj-test.el`
Expected: green — `apply` scalar/symbol/bool/nil results and signal/throw all unchanged.
Run `cargo clippy` — no new warnings.

- [ ] **Step 7: Commit**

```bash
git add src/commands.rs src/lisp_requests.rs src/jvm_thread.rs
git commit -m "refactor: LispOutcome composes BridgeValue for scalars

Drop the duplicated Nil/Int/Float/BigInt/Str/Symbol variants; build the
Value arm through the existing lisp_arg_to_jvalue marshaller. Keep Bool
(computed boolean) and Ptr distinct. No behaviour change."
```

---

## Task 4: First-class `LispSignal`/`LispThrow` classes; remove `ex-info` (Item 3)

Give signals and throws their own platform-loader classes extending `LispNonLocalExit`, so Clojure catches by type and reads fields. This task covers the **Clojure apply reply path** (already structured via `complete_future_with_reply`). The **generic async callback path** stops stringifying in Task 9 (Item 2), where the invoke reply gains a structured outcome — noted there.

**Files:**
- Create: `java/emacs_jni/core/LispSignal.java`, `java/emacs_jni/core/LispThrow.java`.
- Modify: `java/emacs_jni/core/LispNonLocalExit.java` (make non-`final`; it becomes the shared base).
- Modify: the shim class-loading list (bytecode embed) — locate via Step 1.
- Modify: `src/jvm_thread.rs` (`build_non_local_exit` picks the subclass by `kind`).
- Modify: `clojure-src/emacs_jni/clj.clj` (typed `catch`, delete `ex-info` helpers/predicates).
- Test: `test/jvm-clj-test.el` (rewrite the two signal/throw tests to catch by type).

**Interfaces:**
- Consumes: `LispOutcome::Signal`/`Throw` (Task 3), `build_lisp_reply_object` (Task 3).
- Produces: `build_non_local_exit(env, kind, a, b)` builds `LispSignal` when `kind==0`, `LispThrow` when `kind==1`, each still assignable to a `LispNonLocalExit`/`Throwable` catch. Field names on both subclasses: `symbol`/`data` (signal), `tag`/`value` (throw); the base keeps `kind`/`symbolOrTag`/`dataOrValue`.

- [ ] **Step 1: Locate how shim classes are loaded, and confirm the bytecode source**

Run: `grep -rnE 'define_class|include_bytes|LispNonLocalExit|define_classes|\.class' src/jvm_thread.rs src/shim.rs`
Read the `define_classes` function and how each class's bytecode is obtained (either `include_bytes!` of a compiled `.class`, or a build step compiling `java/emacs_jni/core/*.java`). Record the exact mechanism — Steps 2–4 must add `LispSignal`/`LispThrow` to it the same way the existing five classes are added. If classes are compiled from source at build time, adding the two `.java` files may be enough; if `.class` bytes are embedded per-class, add two embed entries and rebuild the fixtures.

Expected: a concrete list of edits needed to make the JVM load the two new classes. (No code change in this step.)

- [ ] **Step 2: Write the failing ERT tests**

Rewrite the two tests in `test/jvm-clj-test.el` (`jvm-clj-test-apply-error-becomes-lisp-signal` at line 149, `jvm-clj-test-throw-becomes-lisp-throw` at line 162) to catch by type and read fields:

```elisp
(ert-deftest jvm-clj-test-apply-error-becomes-lisp-signal ()
  "A Lisp `error' surfaces to Clojure as a LispSignal caught by type,
exposing the signal symbol and data as fields."
  (jvm-clj-test--ensure-ready)
  (should
   (equal
    (jvm-clj-test--eval
     "(try (emacs-jni.clj/apply (emacs-jni.clj/intern \"error\") \"boom\")
        (catch emacs_jni.core.LispSignal e
          (str (emacs-jni.clj/->clj (.-symbol e)))))")
    "error")))

(ert-deftest jvm-clj-test-throw-becomes-lisp-throw ()
  "A Lisp `throw' surfaces to Clojure as a LispThrow caught by type,
exposing tag and value as fields."
  (jvm-clj-test--ensure-ready)
  (should
   (equal
    (jvm-clj-test--eval
     "(let [f (emacs-jni.clj/intern \"eval\")
            form (emacs-jni.clj/apply (emacs-jni.clj/intern \"read\")
                                       \"(throw 'my-tag 42)\")]
        (try (emacs-jni.clj/apply f form)
          (catch emacs_jni.core.LispThrow e
            (str (emacs-jni.clj/->clj (.-tag e)) \"|\" (.-value e)))))")
    "my-tag|42")))
```

(`->clj` is `<-lisp` made public in Step 6 — the fields hold marshalled parts, e.g. a `LispSymbol` marker for the symbol/tag, so tests convert them the same way the runtime does.)

- [ ] **Step 3: Run to verify failure**

Run: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-clj-test.el`
Expected: FAIL — `emacs_jni.core.LispSignal`/`LispThrow` don't exist yet (ClassNotFound), and `emacs-jni.clj/->clj` is undefined.

- [ ] **Step 4: Add the Java classes**

Make the base non-`final` in `java/emacs_jni/core/LispNonLocalExit.java`:

```java
public class LispNonLocalExit extends RuntimeException {
```

Create `java/emacs_jni/core/LispSignal.java`:

```java
package emacs_jni.core;

// A Lisp `signal' crossing to the JVM. Extends the neutral base so a single
// `catch (LispNonLocalExit)' still works; adds typed `symbol'/`data' fields
// (already marshalled: a LispSymbol marker, a LispObject, or a boxed scalar).
public final class LispSignal extends LispNonLocalExit {
    public final Object symbol;
    public final Object data;

    public LispSignal(Object symbol, Object data) {
        super(SIGNAL, symbol, data);
        this.symbol = symbol;
        this.data = data;
    }
}
```

Create `java/emacs_jni/core/LispThrow.java`:

```java
package emacs_jni.core;

// A Lisp `throw' crossing to the JVM. See LispSignal.
public final class LispThrow extends LispNonLocalExit {
    public final Object tag;
    public final Object value;

    public LispThrow(Object tag, Object value) {
        super(THROW, tag, value);
        this.tag = tag;
        this.value = value;
    }
}
```

Add both to the shim class-loading list per Step 1's finding (embed/compile the same way the other core classes are).

- [ ] **Step 5: Build the subclass in Rust**

In `src/jvm_thread.rs`, change `build_non_local_exit` to construct the subclass by `kind` (both are still `LispNonLocalExit`/`Throwable` for the existing catch/rethrow sites):

```rust
fn build_non_local_exit<'local>(
    env: &mut jni::Env<'local>,
    kind: i32,
    a: crate::commands::LispOutcome,
    b: crate::commands::LispOutcome,
) -> jni::errors::Result<jni::objects::JObject<'local>> {
    let a_obj = build_lisp_reply_object(env, a)?;
    let b_obj = build_lisp_reply_object(env, b)?;
    let class = if kind == 0 {
        env.find_class(jni::jni_str!("emacs_jni/core/LispSignal"))?
    } else {
        env.find_class(jni::jni_str!("emacs_jni/core/LispThrow"))?
    };
    env.new_object(
        &class,
        jni::jni_sig!((a: java.lang.Object, b: java.lang.Object) -> void),
        &[jni::objects::JValue::Object(&a_obj), jni::objects::JValue::Object(&b_obj)],
    )
}
```

(`describe_pending_java_exception`'s `getCause`-chain walk for `LispNonLocalExit` at `src/jvm_thread.rs:1040-1051` still matches, since both subclasses ARE `LispNonLocalExit` — verify no `find_class` there needs updating.)

- [ ] **Step 6: Replace `ex-info` with typed catch in Clojure**

In `clojure-src/emacs_jni/clj.clj`:
- Add `LispSignal LispThrow` to the `:import` (line 11).
- Make `<-lisp` public and add a public alias `->clj` for tests/consumers:

```clojure
(defn ->clj
  "Convert a marshalled Lisp value (a LispSymbol marker, LispObject, or
  boxed scalar) to its Clojure form: a marker becomes a symbol/keyword,
  everything else passes through."
  [x]
  (if (instance? LispSymbol x)
    (if (.-keyword x) (keyword (.-name x)) (symbol (.-name x)))
    x))
```

  and replace the private `<-lisp` body with `(->clj x)` (or rename `<-lisp` call sites to `->clj` and delete `<-lisp`).
- Delete `lisp-exit->ex-info` (lines 59-67), `lisp-signal?` (179-182), `lisp-throw?` (184-187).
- Rewrite `throw-lisp-exit` to just unwrap and rethrow the native exception unchanged (callers now catch by type):

```clojure
(defn- throw-lisp-exit
  "Given a completion throwable (a bare LispNonLocalExit from .handle, or one
  wrapped in an ExecutionException from deref), rethrow the underlying
  LispSignal/LispThrow so a caller can `catch' it by type. Anything else is
  rethrown unchanged."
  [t]
  (let [cause (if (instance? java.util.concurrent.ExecutionException t) (.getCause t) t)]
    (throw (if (instance? LispNonLocalExit cause) cause t))))
```

  Leave `apply`, `apply-async`, `run-lisp` calling `throw-lisp-exit` — they now propagate the typed exception. Update their docstrings from "surfaces as an ex-info (::lisp-signal or ::lisp-throw)" to "surfaces as a LispSignal/LispThrow".

- [ ] **Step 7: Run the tests to verify pass**

Run: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-clj-test.el`
Expected: PASS — the two rewritten tests, plus every other `jvm-clj-test` case still green.
Run the direct-path exit test too: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-clj-direct-test.el` (it exercises `throw_non_local_exit`).
Run `cargo test` — 42/42.
Run `cargo clippy` — no new warnings.

- [ ] **Step 8: Commit**

```bash
git add java/emacs_jni/core/LispNonLocalExit.java java/emacs_jni/core/LispSignal.java java/emacs_jni/core/LispThrow.java src/jvm_thread.rs clojure-src/emacs_jni/clj.clj test/jvm-clj-test.el
# plus any shim-loading file from Step 1
git commit -m "feat: first-class LispSignal/LispThrow, drop ex-info

Signals and throws cross as typed subclasses of LispNonLocalExit; Clojure
catches by type and reads symbol/data or tag/value fields. Removes the
ex-info translation and lisp-signal?/lisp-throw? predicates."
```

---

## Task 5: Generalise the inbound queue and add the native drain (Item 2, scaffold)

Turn the `lisp_requests` queue into the single inbound queue for EVERY JVM→Emacs message, and rename the drain `jvm--service-lisp-requests`→`jvm--service-inbound`. This task adds the typed variants and the drain arms **alongside** the still-active text path — nothing is migrated off text yet, so the whole suite stays green. Migration happens one type at a time in Tasks 6–8; deletion of the text protocol is Task 9.

**Files:**
- Modify: `src/lisp_requests.rs` — rename `LispRequest`→`Inbound` (or add an `Inbound` enum wrapping it; see Step 1 decision), add `CallDone`/`Invoke`/`Release`/`Log`/`Status`/`LoadJarDone` variants, extend `service`→`service_inbound`.
- Modify: `src/lib.rs` — rename `jvm--service-lisp-requests`→`jvm--service-inbound` defun (keep the old name as an alias temporarily is unnecessary; update the one Elisp caller in the same task).
- Modify: `elisp/jvm.el` — update the `(lisp-request)` dispatch case to call `jvm--service-inbound` (name change only, still text-triggered for now).
- Test: `cargo test` for the new enum variants' construction; existing suite green.

**Interfaces:**
- Produces: `lisp_requests::Inbound` enum with the existing reply-bearing/FreeRef requests PLUS:
  - `CallDone { id: u64, outcome: BridgeValueOrError }` — where the result is a `BridgeValue` on success or an error string; represent as `CallDone { id: u64, ok: bool, result: BridgeValue, err: Option<String> }` to mirror the current `call_done_message` shape exactly.
  - `Invoke { id: u64, handle: u64, args: Vec<BridgeValue> }`
  - `Release { handle: u64 }`
  - `Log { level: String, text: String }`
  - `Status { status: crate::status::Status, detail: Option<String> }`
  - `LoadJarDone { id: u64, ok: bool, err: Option<String> }`
- Produces: `#[defun] jvm--service-inbound` (drains and dispatches all variants).
- Produces: `lisp_requests::enqueue_inbound(Inbound)` — the enqueue used by producers in Tasks 6–8.

- [ ] **Step 1: Decide the enum shape and add the variants**

In `src/lisp_requests.rs`, rename the existing `enum LispRequest` (line 50) to `enum Inbound` and add the six new variants above. Keep the existing reply-bearing variants (`Intern`/`Apply`/`TypeOf`/`IsNotNil`/`Eq`/`Prin1`/`FreeRef`) unchanged. Rename `QUEUE`'s element type, `enqueue`→keep name (or add `enqueue_inbound`), `drain`, and `service`→`service_inbound` accordingly. Update the reentrant-path callers in `src/shim.rs` (`LispRequest::…`→`Inbound::…`) and `compute_reply`/`run_request_value` signatures.

Add a Rust unit test proving the new variants construct and drain FIFO alongside the old ones (extend the existing `drain_returns_requests_in_fifo_order` test module):

```rust
    #[test]
    fn inbound_status_and_call_done_variants_drain_in_order() {
        enqueue(Inbound::Status { status: crate::status::Status::Ready, detail: None });
        enqueue(Inbound::LoadJarDone { id: 5, ok: true, err: None });
        let drained = drain();
        assert!(matches!(drained[0], Inbound::Status { status: crate::status::Status::Ready, .. }));
        assert!(matches!(drained[1], Inbound::LoadJarDone { id: 5, ok: true, .. }));
    }
```

- [ ] **Step 2: Run to verify it fails to compile / test fails**

Run: `cargo test`
Expected: FAIL — new variants/functions not yet fully wired, or the new test can't compile until Step 1 is complete. Iterate Step 1 until this test is the only red.

- [ ] **Step 3: Add drain arms for the new variants in `service_inbound`**

For this scaffold task the new variants are not yet produced by anyone, so their arms can start as the eventual real handlers (they compile and are covered by Tasks 6–8's tests) OR a `todo!`-free placeholder that logs. Prefer writing the real handler bodies now so Tasks 6–8 only flip the producer:
- `CallDone { id, ok, result, err }` → `env.call("jvm--resolve-call", (id as i64, ok, bridge_value_to_value(env, &result)?, err))` (a small Elisp helper added in Task 6).
- `Invoke { id, handle, args }` → funcall the callback and reply (Task 7 wires this; for now, build the arm calling an Elisp `jvm--resolve-invoke`).
- `Release { handle }` → `env.call("jvm--drop-callback", (handle as i64,))`.
- `Log { level, text }` → `env.call("jvm--log-message", (level.as_str(), text.as_str()))`.
- `Status { status, detail }` → `env.call("jvm--apply-status", (status.as_tag(), detail))`.
- `LoadJarDone { id, ok, err }` → `env.call("jvm--resolve-load-jar", (id as i64, ok, err))`.

Because these Elisp helpers don't exist yet and no producer enqueues these variants yet, gate them behind the task that introduces each (Tasks 6–8). To keep THIS task green without dead `env.call`s to undefined functions, implement each arm's body but do not enqueue any new variant yet — the arms are only reached once a producer enqueues, which happens in later tasks. (Undefined-function calls only error at runtime when reached, so an unreached arm is safe; still, add the Elisp helpers as no-op-safe stubs here to avoid a half-defined interface — see Step 4.)

- [ ] **Step 4: Add Elisp helper stubs and rename the dispatch call**

In `elisp/jvm.el`, rename the `(lisp-request)` pcase case body call from `(jvm--service-lisp-requests)` to `(jvm--service-inbound)`. Add helper functions that Tasks 6–8 will point the real messages at — for now each simply reuses the existing dispatch logic so behaviour is identical:

```elisp
(defun jvm--resolve-call (id ok value err)
  "Resolve the pending `jvm-call-async' callback for ID."
  (when-let ((callback (alist-get id jvm--pending-call)))
    (setq jvm--pending-call (assq-delete-all id jvm--pending-call))
    (funcall callback ok value err)))

(defun jvm--resolve-load-jar (id ok err)
  (when-let ((callback (alist-get id jvm--pending-load-jar)))
    (setq jvm--pending-load-jar (assq-delete-all id jvm--pending-load-jar))
    (funcall callback ok err)))

(defun jvm--apply-status (state detail)
  (setq jvm--status (intern state))
  (when detail (jvm--log "[status] %s: %s" state detail))
  (run-hook-with-args 'jvm-status-hook (intern state)))

(defun jvm--log-message (level text)
  (jvm--log "[%s] %s" level text)
  (message "emacs-jni [%s]: %s" level text))

(defun jvm--drop-callback (handle)
  (remhash handle jvm--lisp-callbacks))
```

(`jvm--resolve-invoke` is added in Task 7.) The existing text `jvm--dispatch-message` cases stay untouched and still handle live traffic — these helpers are dormant until their producer is migrated.

- [ ] **Step 5: Build, test, full ERT suite**

Run: `cargo test`
Expected: PASS including the new FIFO test.
Run the full ERT suite (every file) — expected all green; behaviour is unchanged because no producer emits the new variants yet.
Run `cargo clippy` — no new warnings (an unused `enqueue_inbound`/arm may warrant `#[cfg]`/`#[expect(dead_code)]` with a reason comment naming Tasks 6–8; prefer wiring producers immediately in the next tasks to avoid dead code).

- [ ] **Step 6: Commit**

```bash
git add src/lisp_requests.rs src/lib.rs src/shim.rs elisp/jvm.el
git commit -m "refactor: unified inbound queue + native drain scaffold

Rename LispRequest->Inbound, add CallDone/Invoke/Release/Log/Status/
LoadJarDone variants and jvm--service-inbound. Text path still live; no
message migrated yet."
```

---

## Task 6: Migrate `call-done` and `load-jar-done` to the typed queue (Item 2)

Move the two worker→Lisp result messages off text. The producers (`handle_call`, `handle_load_jar`) enqueue a typed `Inbound` and poke a wakeup; the drain resolves the pending callback. `CallDone` carries the result as a `BridgeValue` converted with the single `lisp_arg_to_value` (renamed `bridge_value_to_value`) — no sexp round trip.

During migration the pipe filter is still the text line-buffered `jvm--process-filter`, which only drains on a newline-terminated recognized line. So the wakeup here MUST be the existing `(lisp-request)` text message (which the live filter already dispatches to `jvm--service-inbound`), NOT a raw byte. The raw-byte `channel::poke()` + `#[defun]` filter land together in Task 9.

**Files:**
- Modify: `src/jvm_thread.rs` (`handle_call:752-758`, `handle_load_jar:337-354`) — enqueue + send the existing `(lisp-request)` wakeup, instead of `channel::send(protocol::call_done_message/load_jar_done_message(…))`.
- Modify: `elisp/jvm.el` — delete the `(call-done …)` and `(load-jar-done …)` text cases; keep the helpers from Task 5.
- Test: existing `test/jvm-call-test.el`, `test/jvm-load-jar-test.el` stay green; add an async keyword/symbol return test.

**Interfaces:**
- Consumes: `Inbound::CallDone`/`LoadJarDone`, `bridge_value_to_value` (was `lisp_arg_to_value`), `protocol::lisp_request_message` (the wakeup, until Task 9 replaces it).

- [ ] **Step 1: Write the failing ERT test (async keyword/symbol return)**

Add to `test/jvm-call-test.el` (mirrors its existing style; needs the Clojure jars for a symbol-producing call, or use `Calc` returning a `LispSymbol` — simplest is the async path returning a keyword via a callback already covered; here assert the async call-done path carries a symbol):

```elisp
(ert-deftest emacs-jni-jvm-call-test/async-call-returns-keyword ()
  "The async call-done path carries a keyword result through the typed
queue (previously only the sexp reify exercised this)."
  (should (eq (jvm-start-await) 'ready))
  (jvm-load-jar-await
   (expand-file-name "../target/test-fixtures/calc.jar"
                     (file-name-directory (or load-file-name buffer-file-name))))
  ;; A Lisp callback returns :ok; Calc.callWithArg returns it; the async
  ;; call-done delivers it as an interned keyword.
  (should (eq (jvm-call-await "Calc" "callWithArg" (list (lambda (_n) :ok) 1)) :ok)))
```

(If `jvm-call-test.el` lacks a jar helper, copy the `--calc-jar` defvar pattern from `jvm-invoke-test.el`.)

- [ ] **Step 2: Run to verify it passes on the OLD path first, then guard the migration**

Run: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-call-test.el`
Expected: with Task 1 merged, this already PASSes on the text path (the callback returns via invoke, and callWithArg's return crosses via call-done → sexp `(jvm-symbol …)` → `:ok`). This test is the regression guard the migration must keep green. Record it passing, then proceed to migrate and re-run.

- [ ] **Step 3: Enqueue instead of formatting text in the producers**

In `src/jvm_thread.rs` `handle_call` (752-758):

```rust
    match result {
        Ok(value) => {
            crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::CallDone {
                id,
                ok: true,
                result: value,
                err: None,
            });
        }
        Err(e) => {
            crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::CallDone {
                id,
                ok: false,
                result: crate::commands::BridgeValue::Nil,
                err: Some(e.to_string()),
            });
        }
    }
    // Wakeup: reuse the existing text (lisp-request) message the live filter
    // already dispatches to jvm--service-inbound. Task 9 swaps this for the
    // raw-byte channel::poke() together with the #[defun] filter.
    channel::send(&protocol::lisp_request_message());
```

In `handle_load_jar` (337-354), replace the two `channel::send(protocol::load_jar_done_message(…))` with `enqueue(Inbound::LoadJarDone { id, ok, err })` and, after, one `channel::send(&protocol::lisp_request_message())`.

The `service_inbound` `CallDone` arm (from Task 5 Step 3): convert `result` via `bridge_value_to_value(env, &result)` and call `jvm--resolve-call`; the `LoadJarDone` arm calls `jvm--resolve-load-jar`.

- [ ] **Step 4: Remove the migrated text cases**

In `elisp/jvm.el` `jvm--dispatch-message`, delete the `` `(call-done …) `` and `` `(load-jar-done …) `` pcase cases (lines 74-81). Keep `jvm--resolve-call`/`jvm--resolve-load-jar` (now driven by the drain). Leave `status`/`log`/`invoke`/`release`/`lisp-request` cases for later tasks.

Note: the `-await` wrappers block on `accept-process-output`, which fires the filter on the `(lisp-request)` wakeup line → `jvm--service-inbound` drains → resolves the pending alist. The blocking-wait loop (`(while (eq result :pending) …)`) is unchanged.

- [ ] **Step 5: Build, test**

Run: `cargo test` — 42/42 (+ any Rust unit change).
Run: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-call-test.el` and `test/jvm-load-jar-test.el`
Expected: green, including the new keyword test.
Run `cargo clippy` — no new warnings.

- [ ] **Step 6: Commit**

```bash
git add src/jvm_thread.rs elisp/jvm.el test/jvm-call-test.el
git commit -m "refactor: call-done/load-jar-done via typed queue

Producers enqueue Inbound::CallDone/LoadJarDone and send the existing
(lisp-request) wakeup; the native drain resolves the pending callback.
Result crosses as BridgeValue through bridge_value_to_value, no sexp round
trip."
```

---

## Task 7: Migrate `invoke` to the typed queue (Item 2) + structured callback exit (Item 3 tail)

The JVM→Lisp callback path (`shim::lisp_function_call` → `(invoke …)`) moves to the typed queue. Args cross as `Vec<BridgeValue>` (no sexp), and the reply carries a structured `LispOutcome` — so an uncaught Lisp exit from a callback surfaces to the JVM as a real `LispSignal`/`LispThrow` instead of a stringified message (the Item 3 tail deferred from Task 4).

**Files:**
- Modify: `src/shim.rs` (`lisp_function_call:196-213`) — enqueue `Inbound::Invoke` + poke, instead of sexp `invoke_message`.
- Modify: `src/shim.rs` (`jobject_array_to_sexp:275-284`) — replace with a `Vec<BridgeValue>` builder (reuse `jvm_value_to_lisp_arg`), delete the sexp version.
- Modify: `src/lib.rs` (`jvm_invoke_reply:243-259`) — the reply's `ok=nil` path carries a structured outcome, not a string.
- Modify: `elisp/jvm.el` — replace the `(invoke …)` text case with `jvm--resolve-invoke` (called from the drain); the callback error branch passes the real signal/throw parts.
- Modify: `src/jvm_thread.rs` (`handle_invoke_reply`) — accept the structured outcome for the error path (feed `complete_future_with_reply` the real `Signal`/`Throw`).
- Test: `test/jvm-invoke-test.el` — keep the existing three; rewrite the error test to assert `LispSignal` by type; add a callback signalling with a symbol/data.

**Interfaces:**
- Consumes: `Inbound::Invoke { id, handle, args: Vec<BridgeValue> }`, `LispOutcome` (Signal/Throw), `throw`/`build_non_local_exit`.
- Produces: `Cmd::InvokeReply` carrying a `LispOutcome` instead of `ok: bool, result: BridgeValue` — OR keep `Cmd::InvokeReply { id, outcome: LispOutcome }`. Update `src/commands.rs` `Cmd::InvokeReply` and `handle_invoke_reply` signature accordingly.

- [ ] **Step 1: Write the failing tests**

In `test/jvm-invoke-test.el`, rewrite `emacs-jni-jvm-invoke-test/lisp-error-propagates-as-java-exception` (line 16) so the exception is the typed `LispSignal` carrying the real symbol, and add a data assertion:

```elisp
(ert-deftest emacs-jni-jvm-invoke-test/lisp-error-propagates-as-lisp-signal ()
  "A failed Lisp callback completes the future exceptionally with a real
`LispSignal' carrying the signal symbol/data — not a stringified message."
  (should (eq (jvm-status) 'ready))
  (jvm-load-jar-await jvm-invoke-test--calc-jar)
  ;; Calc.describeCallbackError catches the future's exception and returns
  ;; the LispSignal's symbol name; add that fixture method (Step 2) or assert
  ;; via the existing jvm-error chain that the class name is LispSignal.
  (let ((err (should-error (jvm-call-await "Calc" "callWithArg"
                                           (list (lambda (_n) (error "boom")) 1))
                           :type 'jvm-error)))
    (should (string-match-p "LispSignal" (cadr err)))))
```

- [ ] **Step 2: Run to verify failure**

Run: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-invoke-test.el`
Expected: FAIL — currently the callback error crosses as a fabricated `LispSignal(error, "boom-string")` built in `handle_invoke_reply`; the class today is the base `LispNonLocalExit`, so `"LispSignal"` won't match until this task builds the subclass with the real symbol.

- [ ] **Step 3: Replace the sexp arg builder with a `Vec<BridgeValue>` builder**

In `src/shim.rs`, replace `jobject_array_to_sexp` with:

```rust
fn jobject_array_to_bridge_values(
    env: &mut jni::Env<'_>,
    args: &JObjectArray<'_>,
) -> jni::errors::Result<Vec<crate::commands::BridgeValue>> {
    let len = args.len(env)?;
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let obj = args.get_element(env, i)?;
        out.push(crate::marshal::jvm_value_to_lisp_arg(env, JValueOwned::Object(obj))?);
    }
    Ok(out)
}
```

In `lisp_function_call`, replace the `args_sexp`/`invoke_message`/`channel::send` block (209-210) with:

```rust
    let args = jobject_array_to_bridge_values(env, &args)?;
    crate::lisp_requests::enqueue(crate::lisp_requests::Inbound::Invoke { id, handle, args });
    // Wakeup via the existing text (lisp-request) message (Task 9 swaps for poke()).
    crate::channel::send(&crate::protocol::lisp_request_message());
```

- [ ] **Step 4: Drain `Invoke` and reply with a structured outcome**

The `service_inbound` `Invoke` arm converts each `BridgeValue` arg via `bridge_value_to_value`, then calls `jvm--resolve-invoke`:

```elisp
(defun jvm--resolve-invoke (id handle args)
  "Funcall the callback for HANDLE with ARGS; reply to the JVM with the
result, or the real signal/throw parts on a non-local exit."
  (let ((fn (gethash handle jvm--lisp-callbacks)))
    (if (null fn)
        (jvm--invoke-reply-error id 'error (format "no such callback handle %s" handle))
      (condition-case err
          (jvm--invoke-reply id t (apply fn args))
        ;; `err' is (SYMBOL . DATA); pass the real parts, not a string.
        (error (jvm--invoke-reply-error id (car err) (cdr err)))))))
```

Rework the reply defuns so the error path carries structured parts. In `src/lib.rs`, keep `jvm--invoke-reply` for success (a `BridgeValue` via `lisp_value_to_arg`) and add `jvm--invoke-reply-error` taking a symbol + data, marshalling both to a `LispOutcome::Signal`. Simpler: make one `#[defun] jvm--invoke-reply` that takes `(id ok value)` where, for `ok=nil`, `value` is the `(SYMBOL . DATA)` cons — convert to `LispOutcome::Signal { symbol: Value(Symbol), data: <marshalled> }`. Choose the shape that matches how `complete_future_with_reply` already consumes `LispOutcome` (reuse it). Update `Cmd::InvokeReply` to `{ id, outcome: LispOutcome }` and `handle_invoke_reply` to call `complete_future_with_reply(env, &future_local, outcome)` for both success and exit, deleting the string-only error branch (516-529).

(Design note for the implementer: the cleanest form is `Cmd::InvokeReply { id, outcome: LispOutcome }`. Success = `LispOutcome::Value(bridge)`/`Bool`/`Ptr`; failure = `LispOutcome::Signal`/`Throw`. This makes the callback path identical to the Clojure apply path — one `complete_future_with_reply`. Marshal the Lisp `(SYMBOL . DATA)` on the main thread inside the drain, where an `Env` is live, into the `LispOutcome::Signal` before sending the `Cmd`.)

- [ ] **Step 5: Remove the `(invoke …)` text case and sexp plumbing**

In `elisp/jvm.el`, delete the `` `(invoke …) `` pcase case (82-103). Delete `protocol::invoke_message` usage. Keep `jvm--resolve-invoke`.

- [ ] **Step 6: Build, test**

Run: `cargo test` — 42/42.
Run: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-invoke-test.el` and `test/jvm-callback-wrap-test.el`
Expected: green, including the rewritten `LispSignal` assertion and the real-object-argument regression test (args now cross as `BridgeValue`, still arriving as a `user-ptr`).
Run `cargo clippy` — no new warnings.

- [ ] **Step 7: Commit**

```bash
git add src/shim.rs src/lib.rs src/commands.rs src/jvm_thread.rs elisp/jvm.el test/jvm-invoke-test.el
git commit -m "refactor: invoke via typed queue, structured callback exit

Callback args cross as Vec<BridgeValue>; the reply carries a LispOutcome so
an uncaught Lisp exit surfaces to the JVM as a real LispSignal/LispThrow
instead of a stringified message."
```

---

## Task 8: Migrate `release`, `status`, `log` to the typed queue (Item 2)

The last three text message types. `release` (from `ReleaseNotifier.run`, a Cleaner thread) and the worker's `status`/`log` sends move to `enqueue` + the existing `(lisp-request)` text wakeup (raw-byte `poke()` lands in Task 9). The `(lisp-request)` pcase case STAYS — it is the wakeup dispatcher every migrated type still relies on until Task 9.

**Files:**
- Modify: `src/shim.rs` (`release_notifier_run:250-254`) — enqueue `Inbound::Release` + wakeup.
- Modify: `src/jvm_thread.rs` — every `channel::send(protocol::status_message(…))` (lines 24,29,105,262,294,298,312,316) and `channel::send(protocol::log_message(…))` (238,270,489,539,556,564; and `lisp_requests.rs:314-317`) → `enqueue(Inbound::Status/Log)` + `channel::send(&protocol::lisp_request_message())`.
- Modify: `elisp/jvm.el` — delete ONLY the `(status …)`, `(log …)`, `(release …)` cases; KEEP `(lisp-request)`.
- Test: `test/jvm-start-test.el`, `test/jvm-stop-test.el`, `test/jvm-pause-test.el` (status transitions) stay green.

**Interfaces:**
- Consumes: `Inbound::Release`/`Status`/`Log`, the `jvm--drop-callback`/`jvm--apply-status`/`jvm--log-message` helpers (Task 5), `protocol::lisp_request_message` (wakeup).

- [ ] **Step 1: Guard test — status transitions still fire**

Run the status suite on the current (pre-migration) tree to record green:
`./test/run-ert-tests.sh test/jvm-start-test.el` and `test/jvm-stop-test.el` and `test/jvm-pause-test.el`
Expected: green. These are the regression guard — the `jvm-status-hook` and `jvm--status` transitions must survive the migration.

- [ ] **Step 2: Migrate the producers**

- `release_notifier_run` (`src/shim.rs:250-254`): `enqueue(Inbound::Release { handle })` + `channel::send(&protocol::lisp_request_message())`.
- All `status_message` sends in `src/jvm_thread.rs`: `enqueue(Inbound::Status { status, detail })` + `channel::send(&protocol::lisp_request_message())`. `detail` is the `Option<&str>`→`Option<String>` currently passed.
- All `log_message` sends in `src/jvm_thread.rs` and the one in `src/lisp_requests.rs:314-317` (the `FreeRef` free-failure log): `enqueue(Inbound::Log { level, text })` + `channel::send(&protocol::lisp_request_message())`.

Watch the `run` entry path: the very first `status_message(Ready)` at line 24 fires before any pipe consumer might be attached — but the queue holds it and the first `accept-process-output` drains it, same as today's buffered text. Confirm `jvm-start-await`'s poll still observes `ready` (Step 4).

- [ ] **Step 3: Delete the migrated text cases (keep `(lisp-request)`)**

In `elisp/jvm.el`, delete the `` `(status …) ``, `` `(log …) ``, `` `(release …) `` pcase cases. KEEP the `` `(lisp-request) `` case — it is still the wakeup dispatcher for every enqueued message until Task 9 swaps the filter. After this task `jvm--dispatch-message` has only the `(lisp-request)` and `_` fallback cases left.

- [ ] **Step 4: Build, test**

Run: `cargo test` — 42/42.
Run the status suite + a Clojure suite that exercises `release` (GC of a `LispFunction`), e.g. `EMACS_JNI_CLOJURE_JARS_DIR=… ./test/run-ert-tests.sh test/jvm-clj-test.el`.
Expected: all green — status transitions and log messages still arrive.
Run `cargo clippy` — no new warnings.

- [ ] **Step 5: Commit**

```bash
git add src/shim.rs src/jvm_thread.rs src/lisp_requests.rs elisp/jvm.el
git commit -m "refactor: release/status/log via typed queue

Last inbound payloads move to Inbound; only the (lisp-request) wakeup line
still rides the text filter, removed in the next task."
```

---

## Task 9: Delete the text protocol and switch to the `#[defun]` filter (Item 2, final)

Nothing writes text now. Remove `src/protocol.rs`, the sexp marshaller, the wire tags, and the Elisp line-buffering filter + dispatch; make `jvm--service-inbound` the process filter directly (Q4).

**Files:**
- Modify: `src/channel.rs` — add `poke()` (raw wakeup byte).
- Modify: `src/jvm_thread.rs`, `src/shim.rs`, `src/lisp_requests.rs` — replace every `channel::send(&protocol::lisp_request_message())` wakeup with `channel::poke()`.
- Delete: `src/protocol.rs`; remove `mod protocol;` (`src/lib.rs:10`) and every `use crate::protocol`/`crate::protocol::` (now unused).
- Delete: `src/marshal.rs` `lisp_arg_to_sexp` (427-445), `quote_lisp_string` (452-463), and their `#[cfg(test)]` tests (469-519).
- Modify: `elisp/jvm.el` — delete `jvm--dispatch-message` (incl. the `(lisp-request)` case), `jvm--resolve-sexp-result`, `jvm--process-filter`, `jvm--filter-buffer`; set the pipe filter to `#'jvm--service-inbound`.
- Modify: `src/lib.rs` — `jvm--service-inbound` defun signature accepts `(_proc, _data)` so it is a valid process filter.
- Modify: `test/jvm-symbol-marshal-test.el` — delete `reify-wire-tag` (tests the removed `jvm--resolve-sexp-result`); keep the round-trip test.

**Interfaces:**
- Produces: `channel::poke()` — one raw wakeup byte, no payload.
- Produces: `#[defun] jvm--service-inbound(env, _proc: Value, _data: Value) -> Result<()>` — set via `(set-process-filter … #'jvm--service-inbound)`.

- [ ] **Step 1: Make `jvm--service-inbound` a filter-shaped defun**

In `src/lib.rs`, change the defun to accept and ignore the two filter args:

```rust
#[defun(mod_in_name = false, name = "jvm--service-inbound")]
fn jvm_service_inbound(env: &Env, _process: Value, _data: Value) -> Result<()> {
    lisp_requests::service_inbound(env)
}
```

- [ ] **Step 2: Add `channel::poke()` and replace every text wakeup with it**

In `src/channel.rs`, add:

```rust
/// Wakes the Emacs main thread to drain the inbound queue, with no payload.
/// The process filter (`jvm--service-inbound`) ignores the byte and drains
/// the whole queue, so coalesced wakeups are harmless.
pub fn poke() {
    if let Some(writer) = WRITER.lock().unwrap().as_mut() {
        let _ = writer.write_all(b"\x01");
    }
}
```

Replace every `channel::send(&protocol::lisp_request_message())` / `crate::channel::send(&crate::protocol::lisp_request_message())` call — the migrated-producer wakeups added in Tasks 6–8, PLUS the pre-existing lisp-access wakeup at `src/shim.rs:406` (`service_lisp_request_sync`) and any GC/`FreeRef` producer at `src/shim.rs:585,606` — with `channel::poke()` (or `crate::channel::poke()` per the call site's path). `grep -rn 'lisp_request_message' src` must return nothing after this step.

- [ ] **Step 3: Point the pipe at the native filter and delete the text filter**

In `elisp/jvm.el` `jvm--ensure-pipe-process`, set `:filter #'jvm--service-inbound` (replacing `#'jvm--process-filter`). Delete `jvm--process-filter`, `jvm--filter-buffer` (defvar line 45), `jvm--dispatch-message` in full (including the now-final `(lisp-request)` and `_` cases), and `jvm--resolve-sexp-result` (121-128).

- [ ] **Step 4: Delete `protocol.rs` and the sexp marshaller**

Delete `src/protocol.rs`. Remove `mod protocol;` from `src/lib.rs` and every `use crate::protocol` / `protocol::` reference (all gone after Step 2; `cargo build` will name any stragglers). Delete `lisp_arg_to_sexp`, `quote_lisp_string`, and their tests from `src/marshal.rs`. Remove the now-unused `crate::marshal::lisp_arg_to_sexp` mentions in doc comments.

- [ ] **Step 5: Trim the symbol-marshal test**

In `test/jvm-symbol-marshal-test.el`, delete `emacs-jni-jvm-symbol-marshal/reify-wire-tag` (it calls the removed `jvm--resolve-sexp-result`) and the file-header comment's "pure wire-reify arm" clause. Keep `identity-round-trips-symbols`.

- [ ] **Step 6: Confirm no wire tags or text protocol survive**

Run: `grep -rnE 'jvm-handle|jvm-symbol|jvm--resolve-sexp-result|jvm--dispatch-message|jvm--process-filter|lisp_arg_to_sexp|call_done_message|invoke_message|release_message|status_message|log_message|lisp_request_message|mod protocol' src elisp`
Expected: NO matches (empty output). Any hit is a straggler to remove.

- [ ] **Step 7: Build, full suite, clippy**

Run: `cargo test` — expected pass (protocol.rs's ~13 unit tests are gone; total drops accordingly — that is expected, not a regression).
Run the FULL ERT suite (every `test/*.el`, Clojure files with the jars env var).
Expected: all green — start/stop/pause, call (sync+async), invoke, callback-wrap, clj, clj-eval (both), symbol-marshal, direct paths.
Run `cargo clippy` — no new warnings; the 2 known `jvm_thread.rs` warnings may remain.

- [ ] **Step 8: Commit**

```bash
git add src/channel.rs src/protocol.rs src/lib.rs src/marshal.rs src/jvm_thread.rs src/shim.rs src/lisp_requests.rs elisp/jvm.el test/jvm-symbol-marshal-test.el
git commit -m "refactor: delete text protocol, native inbound filter

Remove src/protocol.rs, lisp_arg_to_sexp, the (jvm-handle …)/(jvm-symbol …)
wire tags, and the Elisp line-buffered filter/dispatch. jvm--service-inbound
is the process filter; the pipe carries only wakeups."
```

---

## Self-Review

**Spec coverage:**
- Item 1 (merge twins + keyword bug) → Task 1. ✓
- Item 2 (remove text protocol, native inbound dispatch) → Tasks 5–9, staged per message type (`call-done`/`load-jar-done` → `invoke` → `release`/`status`/`log` → delete). Filter form = `#[defun]` (Q4) in Task 9. `load-jar-done` gap covered. ✓
- Item 3 (first-class `LispSignal`/`LispThrow`, remove `ex-info`) → Task 4 (apply path + Clojure catch + class), Task 7 (generic callback structured exit). ✓
- Item 4 (rename + dedup) → Task 2 (rename), Task 3 (dedup onto `BridgeValue`). ✓
- Item 5 (OPTIONAL) → deferred by constraint; no task. ✓
- Decisions: enum names (Task 2), `ex-info` removed (Task 4), text fully deleted (Task 9), `#[defun]` filter (Task 9). ✓

**Placeholder scan:** Task 5 Step 3 and Task 7 Step 4 carry a design note where the exact `Cmd::InvokeReply` shape is settled by the implementer against `complete_future_with_reply`; the recommended shape (`{ id, outcome: LispOutcome }`) is stated with concrete code direction, not left open. Task 4 Step 1 requires locating the shim class-loading mechanism before editing — a real investigation step, not a placeholder. No `TODO`/`fill in`/"add error handling" placeholders remain.

**Type consistency:** `BridgeValue`/`LispInput`/`LispOutcome` used consistently from Task 2 on. `Inbound` enum + `service_inbound` + `enqueue` + `channel::poke` named consistently across Tasks 5–9. `bridge_value_to_value` is the renamed `lisp_arg_to_value` (`src/lisp_requests.rs:131`) — rename it in Task 2 alongside the enum rename (add to Task 2 Step 3's sweep) or in Task 5; it is first *used by the drain* in Task 6, so ensure the rename lands no later than Task 6. `->clj`/`<-lisp` reconciled in Task 4 Step 6.

> Implementer note: fold the `lisp_arg_to_value`→`bridge_value_to_value` and `value_to_reply`→`value_to_outcome` function renames into Task 2's mechanical sweep so later tasks reference the final names. If deferred, do them at the top of Task 5.
