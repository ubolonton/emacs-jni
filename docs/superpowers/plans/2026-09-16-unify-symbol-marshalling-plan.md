# Unified symbol/keyword marshalling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the generic JVM bridge marshal Lisp keywords/symbols both directions (via the existing `LispSymbol` marker), so `(jvm-clj-eval-string ":a")` returns `:a` instead of an opaque pointer, and a keyword/symbol may be passed to any `jvm-call*`.

**Architecture:** Add a `LispArg::Symbol` variant carried through the generic bridge. The core learns only its own `emacs_jni.core.LispSymbol` marker class (not `clojure.lang.*`); the Clojure layer converts a Clojure keyword/symbol to that marker via `->lisp`. `jvm-clj-eval-string` routes through a new `emacs-jni.clj/eval-for-lisp` so its result carries the marker.

**Tech Stack:** Rust (jni 0.22.4, emacs 0.21.0 crates), Emacs Lisp (ERT), Clojure 1.12.

**Spec:** `docs/superpowers/specs/2026-09-16-unify-symbol-marshalling-design.md`

## Global Constraints

- `cargo clippy` and `cargo fmt` before committing (project rule). No `.unwrap()`, `panic!`, `unreachable!` in new code paths beyond existing patterns; prefer `if let`.
- Rust: debug profile only (never `--release`). Run specific tests, not the whole suite where possible.
- ERT integration tests run via `./test/run-ert-tests.sh test/<file>.el` (builds the module, one `emacs --batch` per file).
- Clojure integration tests need `EMACS_JNI_CLOJURE_JARS_DIR` set; jars already fetched at `target/clojure-jars`. Prefix commands with `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars"`.
- `name` in a `LispArg::Symbol` is the bare name (no leading `:`); a keyword sets `keyword: true`.
- Prose in code/comments/commits: Simplified Technical English, minimal words.

---

## Baseline (do first, no commit)

Confirm a green starting point so later failures are attributable.

- [ ] **Rust build + unit tests:** `cargo test` → all pass.
- [ ] **Symbol integration baseline:** `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-clj-eval-test.el test/jvm-clj-eval-direct-test.el` → all pass (or skip cleanly).

---

## Task 1: `LispArg::Symbol` variant + all Rust marshalling arms

Adding the variant forces every exhaustive `LispArg` match to handle it, so all Rust marshalling changes land in one compilable commit. The pure `lisp_arg_to_sexp` render is the unit-test gate; the JVM-touching arms are covered by ERT tasks 3–4.

**Files:**
- Modify: `src/commands.rs` (add variant, ~line 31)
- Modify: `src/marshal.rs` (`lisp_arg_to_jvalue` ~line 92; `object_to_lisp_arg` ~line 380; `lisp_arg_to_sexp` ~line 405; unit test in `mod tests`)
- Modify: `src/lisp_requests.rs` (`lisp_arg_to_value` ~line 145)

**Interfaces:**
- Produces: `commands::LispArg::Symbol { name: String, keyword: bool }`.
- Produces wire form: `(jvm-symbol "NAME" t)` for a keyword, `(jvm-symbol "NAME" nil)` for a plain symbol.

- [ ] **Step 1: Add the variant** in `src/commands.rs`, after the `Callback(u64)` arm of `enum LispArg`:

```rust
    /// An interned Lisp symbol or keyword, crossing as an
    /// `emacs_jni.core.LispSymbol` marker (the same class the Clojure
    /// Lisp-access path uses). `name` is the bare name (no leading `:`).
    Symbol { name: String, keyword: bool },
```

- [ ] **Step 2: Write the failing unit test** in `src/marshal.rs`, inside `mod tests` (near `renders_object_handle`):

```rust
    #[test]
    fn renders_symbol_and_keyword() {
        assert_eq!(
            lisp_arg_to_sexp(&LispArg::Symbol { name: "foo".to_string(), keyword: false }),
            "(jvm-symbol \"foo\" nil)"
        );
        assert_eq!(
            lisp_arg_to_sexp(&LispArg::Symbol { name: "foo".to_string(), keyword: true }),
            "(jvm-symbol \"foo\" t)"
        );
    }
```

- [ ] **Step 3: Run it — expect a COMPILE error** (non-exhaustive match), not a test failure yet:

Run: `cargo test renders_symbol_and_keyword 2>&1 | head -30`
Expected: compile errors — `LispArg::Symbol` not covered in `lisp_arg_to_jvalue`, `object_to_lisp_arg` (it produces, not matches — no error there), `lisp_arg_to_sexp`, `lisp_arg_to_value`.

- [ ] **Step 4: Add the `lisp_arg_to_sexp` arm** in `src/marshal.rs` (`match arg`), after the `Callback` arm:

```rust
        LispArg::Symbol { name, keyword } => {
            format!("(jvm-symbol {} {})", quote_lisp_string(name), if *keyword { "t" } else { "nil" })
        }
```

- [ ] **Step 5: Add the `lisp_arg_to_jvalue` arm** in `src/marshal.rs`. Add this arm to the `match (arg, param_name.as_str())` block (place it near the `Callback` arm; the `_` param pattern means it matches for any parameter type, same as `Callback`/`Object`):

```rust
        // A Lisp symbol/keyword crosses into Java as the platform-loader
        // `LispSymbol` marker (identical construction to
        // `jvm_thread::build_lisp_reply_object`'s `Symbol` arm), so the
        // Clojure layer's `<-lisp` and any other consumer read it the same
        // way regardless of which direction produced it.
        (LispArg::Symbol { name, keyword }, _) => {
            let jname = env.new_string(name)?;
            let class = env.find_class(jni::jni_str!("emacs_jni/core/LispSymbol"))?;
            let obj = env.new_object(
                &class,
                jni::jni_sig!((name: java.lang.String, keyword: boolean) -> void),
                &[
                    JValueOwned::Object(jname.into()).borrow(),
                    JValueOwned::Bool(*keyword).borrow(),
                ],
            )?;
            Ok(JValueOwned::Object(obj))
        }
```

- [ ] **Step 6: Add the `object_to_lisp_arg` recognition** in `src/marshal.rs`. Just before the final `let global = env.new_global_ref(&obj)?;` fallback, add:

```rust
    let lisp_symbol_class = env.find_class(jni::jni_str!("emacs_jni/core/LispSymbol"))?;
    if env.is_instance_of(&obj, &lisp_symbol_class)? {
        let name = env
            .get_field(&obj, jni::jni_str!("name"), jni::jni_sig!(java.lang.String))?
            .l()?;
        let name = env.cast_local::<jni::objects::JString>(name)?.to_string();
        let keyword = env
            .get_field(&obj, jni::jni_str!("keyword"), jni::jni_sig!(boolean))?
            .z()?;
        return Ok(LispArg::Symbol { name, keyword });
    }
```

- [ ] **Step 7: Add the `lisp_arg_to_value` arm** in `src/lisp_requests.rs` (`match arg`), before the `Callback` unreachable arm:

```rust
        // Intern on the main thread, mirroring the `Intern` request
        // (`arg_to_value`): a keyword takes its leading `:`, a plain symbol
        // its bare name. `env.intern` uses the global obarray, so the result
        // is `eq`-stable.
        LispArg::Symbol { name, keyword } => {
            if *keyword {
                env.intern(&format!(":{name}"))
            } else {
                env.intern(name)
            }
        }
```

- [ ] **Step 8: Run the unit test — expect PASS:**

Run: `cargo test renders_symbol_and_keyword -- --nocapture`
Expected: PASS.

- [ ] **Step 9: fmt + clippy + full unit tests:**

Run: `cargo fmt && cargo clippy --all-targets 2>&1 | tail -20 && cargo test 2>&1 | tail -20`
Expected: no clippy warnings, all unit tests pass.

- [ ] **Step 10: Commit:**

```bash
git add src/commands.rs src/marshal.rs src/lisp_requests.rs
git commit -m "feat: LispArg::Symbol — symbol/keyword through the generic bridge"
```

---

## Task 2: Async-wire reify arm (Elisp)

The async (`-await`) OUT path parses the wire sexp and reifies tags in `jvm--resolve-sexp-result`. Add the `jvm-symbol` arm. Testable in isolation (no running JVM) with a pure ERT.

**Files:**
- Modify: `elisp/jvm.el` (`jvm--resolve-sexp-result`, ~line 124)
- Create: `test/jvm-symbol-marshal-test.el`

**Interfaces:**
- Consumes: wire tag `(jvm-symbol "NAME" t|nil)` from Task 1.
- Produces: `(jvm--resolve-sexp-result '(jvm-symbol "foo" t))` → the keyword `:foo`; `... nil)` → the symbol `foo`.

- [ ] **Step 1: Write the failing test** — create `test/jvm-symbol-marshal-test.el`:

```elisp
;;; jvm-symbol-marshal-test.el --- Tests for symbol/keyword marshalling -*- lexical-binding: t; -*-

;; Two layers: the pure wire-reify arm (`jvm--resolve-sexp-result', no JVM),
;; and the generic bidirectional round trip through `clojure.core/identity'
;; (Lisp keyword/symbol -> LispSymbol marker -> Java -> back), which needs a
;; running JVM + the Clojure jars.

(require 'ert)
(require 'jvm)
(require 'jvm-clj)

(ert-deftest emacs-jni-jvm-symbol-marshal/reify-wire-tag ()
  "The `jvm-symbol' wire tag reifies to an interned keyword/symbol."
  (should (eq (jvm--resolve-sexp-result '(jvm-symbol "foo" t)) :foo))
  (should (eq (jvm--resolve-sexp-result '(jvm-symbol "foo" nil)) 'foo)))

(provide 'jvm-symbol-marshal-test)
;;; jvm-symbol-marshal-test.el ends here
```

- [ ] **Step 2: Run it — expect FAIL** (the arm doesn't exist yet, so `(jvm-symbol "foo" t)` falls through `_` and returns the list unchanged, failing the `eq`):

Run: `./test/run-ert-tests.sh test/jvm-symbol-marshal-test.el 2>&1 | tail -20`
Expected: `reify-wire-tag` FAILS.

- [ ] **Step 3: Add the arm** in `elisp/jvm.el`, in `jvm--resolve-sexp-result`'s `pcase`, before the `(_ result)` arm:

```elisp
    (`(jvm-symbol ,name ,keyword)
     (intern (if keyword (concat ":" name) name)))
```

- [ ] **Step 4: Run it — expect PASS:**

Run: `./test/run-ert-tests.sh test/jvm-symbol-marshal-test.el 2>&1 | tail -20`
Expected: `reify-wire-tag` PASSES (the second, JVM-backed test is added in Task 3; it does not exist yet).

- [ ] **Step 5: Commit:**

```bash
git add elisp/jvm.el test/jvm-symbol-marshal-test.el
git commit -m "feat: reify jvm-symbol wire tag to a Lisp keyword/symbol"
```

---

## Task 3: IN dispatch — accept keywords/symbols (Rust) + generic round-trip test

`lisp_value_to_arg` today signals on a keyword/non-`t` symbol. Make a symbol data (a marker) and reserve `Callback` for genuine function *objects*: reorder so `symbolp` precedes `functionp`. Verified end-to-end by echoing through `clojure.core/identity` (IN builds the marker, OUT recognises it — Task 1's direct `lisp_arg_to_value`).

**Files:**
- Modify: `src/lib.rs` (`lisp_value_to_arg`, lines ~318-381)
- Modify: `test/jvm-symbol-marshal-test.el` (add the JVM round-trip test)

**Interfaces:**
- Consumes: `LispArg::Symbol` (Task 1), the direct OUT converter (Task 1).
- Produces: `(jvm-call-direct IFN "invoke" (list :a))` → `:a`; `'foo` → `foo`; `'car` → `car`.

- [ ] **Step 1: Write the failing test** — append to `test/jvm-symbol-marshal-test.el` before the `(provide ...)`:

```elisp
(defvar jvm-symbol-marshal-test--jars-dir (getenv "EMACS_JNI_CLOJURE_JARS_DIR"))
(defvar jvm-symbol-marshal-test--ready nil)

(defun jvm-symbol-marshal-test--ensure-ready ()
  (unless (and jvm-symbol-marshal-test--jars-dir
               (file-directory-p jvm-symbol-marshal-test--jars-dir))
    (ert-skip "Set EMACS_JNI_CLOJURE_JARS_DIR (see test/fetch-clojure-jars.sh) to run this test"))
  (unless jvm-symbol-marshal-test--ready
    (should (eq (jvm-start-await) 'ready))
    (dolist (jar (directory-files jvm-symbol-marshal-test--jars-dir t "\\.jar\\'"))
      (jvm-load-jar-await jar))
    (setq jvm-symbol-marshal-test--ready t)))

(ert-deftest emacs-jni-jvm-symbol-marshal/identity-round-trips-symbols ()
  "A Lisp keyword/symbol passed to `clojure.core/identity' via the generic
bridge crosses as a marker and comes back as the same interned value. A
function symbol (`car') is data here, not a callback."
  (jvm-symbol-marshal-test--ensure-ready)
  (let ((identity-fn (jvm-call-direct "clojure.java.api.Clojure" "var"
                                      (list "clojure.core" "identity"))))
    (should (eq (jvm-call-direct identity-fn "invoke" (list :a)) :a))
    (should (eq (jvm-call-direct identity-fn "invoke" (list 'foo)) 'foo))
    (should (eq (jvm-call-direct identity-fn "invoke" (list 'car)) 'car))))
```

- [ ] **Step 2: Run it — expect FAIL** (IN still signals on a keyword/symbol):

Run: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-symbol-marshal-test.el 2>&1 | tail -25`
Expected: `identity-round-trips-symbols` FAILS with a `jvm-invalid-argument` signal (the reify unit test still passes).

- [ ] **Step 3: Add the keyword arm** in `src/lib.rs` `lisp_value_to_arg`, right after the leading `if !v.is_not_nil()` nil check:

```rust
    if v.env.call("keywordp", (v,))?.is_not_nil() {
        let name: String = v.env.call("symbol-name", (v,))?.into_rust()?;
        // `symbol-name` of a keyword includes its leading `:`; strip it so
        // `name` is the bare name the marker carries.
        let name = name.strip_prefix(':').unwrap_or(&name).to_string();
        return Ok(commands::LispArg::Symbol { name, keyword: true });
    }
```

- [ ] **Step 4: Move the symbol arm before `functionp`.** In `src/lib.rs`, keep the existing `t` check (it must stay before the generic `symbolp`). After the `t` → `True` arm and BEFORE the `functionp` arm, insert:

```rust
    // A symbol is data: it crosses as a marker, not a callback. Checked
    // before `functionp` so a symbol holding a function cell (`car`) is a
    // marker too; only a non-symbol function object (lambda, closure,
    // byte-code, subr) reaches the `functionp` arm below. Matches the
    // Clojure `->lisp` model, where every symbol is a `LispSymbol` marker
    // and callability is opt-in via `require-symbol`.
    if v.env.call("symbolp", (v,))?.is_not_nil() {
        let name: String = v.env.call("symbol-name", (v,))?.into_rust()?;
        return Ok(commands::LispArg::Symbol { name, keyword: false });
    }
```

Leave the existing `functionp` → `Callback` arm and the final `signal` arm in place, unchanged.

- [ ] **Step 5: fmt + clippy:**

Run: `cargo fmt && cargo clippy --all-targets 2>&1 | tail -20`
Expected: no warnings.

- [ ] **Step 6: Run the test — expect PASS:**

Run: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-symbol-marshal-test.el 2>&1 | tail -25`
Expected: both `reify-wire-tag` and `identity-round-trips-symbols` PASS.

- [ ] **Step 7: Regression — the callback path (lambda) is unchanged:**

Run: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-callback-wrap-test.el 2>&1 | tail -15`
Expected: PASS (a `lambda` is a function object, not a symbol, so it still becomes a callback).

- [ ] **Step 8: Commit:**

```bash
git add src/lib.rs test/jvm-symbol-marshal-test.el
git commit -m "feat: pass Lisp keywords/symbols into jvm-call as markers"
```

---

## Task 4: eval-string routes through the Clojure layer

`clojure.core/eval` of `:a` returns a `clojure.lang.Keyword` the core does not recognise. Route eval-string through `emacs-jni.clj/eval-for-lisp` (`(->lisp (eval form))`) so a keyword/symbol result becomes a marker. Update both eval test files' `ensure-ready` (eval-string now needs `emacs-jni.clj` loaded).

**Files:**
- Modify: `clojure/emacs_jni/clj.clj` (add `eval-for-lisp`, after `intern`, ~line 42)
- Modify: `elisp/jvm-clj.el` (`jvm-clj--invoke-with-form-direct` line 84-86 caller; `jvm-clj-eval-string-direct` line 108; `jvm-clj-eval-string-await` line 59 — change the resolved var)
- Modify: `test/jvm-clj-eval-test.el` (`ensure-ready`, line 23)
- Modify: `test/jvm-clj-eval-direct-test.el` (`ensure-ready`, line 17; add keyword/symbol eval tests)

**Interfaces:**
- Consumes: `->lisp` (private, same namespace); Task 1's OUT converters; Task 3 not required.
- Produces: `(jvm-clj-eval-string ":a")` → `:a`; `"'foo"` → `foo`.

- [ ] **Step 1: Add `eval-for-lisp`** in `clojure/emacs_jni/clj.clj`, after the `intern` defn (line 42):

```clojure
(defn eval-for-lisp
  "Evaluate FORM and marshal the result toward Lisp: a keyword or symbol
  result becomes a LispSymbol marker the generic bridge interns back into a
  Lisp keyword/symbol; every other value passes through the ordinary
  jvm-call return marshalling. Backs `jvm-clj-eval-string'."
  [form]
  (->lisp (eval form)))
```

- [ ] **Step 2: Point eval-string at the new var.** In `elisp/jvm-clj.el`, change `jvm-clj-eval-string-direct` (line 108) from:

```elisp
  (jvm-clj--invoke-with-form-direct "clojure.core" "eval" code))
```

to:

```elisp
  (jvm-clj--invoke-with-form-direct "emacs-jni.clj" "eval-for-lisp" code))
```

and `jvm-clj-eval-string-await` (line 59) from:

```elisp
  (jvm-clj--invoke-with-form-await "clojure.core" "eval" code timeout))
```

to:

```elisp
  (jvm-clj--invoke-with-form-await "emacs-jni.clj" "eval-for-lisp" code timeout))
```

(Leave the require paths and `jvm-clj-require-*` untouched — only eval changes.)

- [ ] **Step 3: Update `ensure-ready` in `test/jvm-clj-eval-direct-test.el`** (line 21-25). Replace the `unless ...ready` body so it also loads the source dir and requires the namespace:

```elisp
  (unless jvm-clj-eval-direct-test--ready
    (should (eq (jvm-start-await) 'ready))
    (dolist (jar (directory-files jvm-clj-eval-direct-test--jars-dir t "\\.jar\\'"))
      (jvm-load-jar-await jar))
    (jvm-load-jar-await jvm-clj--source-dir)
    (jvm-clj-require-await "emacs-jni.clj")
    (setq jvm-clj-eval-direct-test--ready t)))
```

- [ ] **Step 4: Update `ensure-ready` in `test/jvm-clj-eval-test.el`** (line 25-29). Replace the `unless ...ready` body the same way:

```elisp
  (unless jvm-clj-eval-test--ready
    (should (eq (jvm-start-await) 'ready))
    (dolist (jar (directory-files jvm-clj-eval-test--jars-dir t "\\.jar\\'"))
      (jvm-load-jar-await jar))
    (jvm-load-jar-await jvm-clj--source-dir)
    (jvm-clj-require-await "emacs-jni.clj")
    (setq jvm-clj-eval-test--ready t)))
```

- [ ] **Step 5: Write the failing tests** — add to `test/jvm-clj-eval-direct-test.el` before its `(provide ...)`:

```elisp
(ert-deftest emacs-jni-jvm-clj-eval-direct-test/eval-returns-keyword ()
  "A Clojure keyword result comes back as a real Lisp keyword, not a pointer."
  (jvm-clj-eval-direct-test--ensure-ready)
  (should (eq (jvm-clj-eval-string-direct ":a") :a)))

(ert-deftest emacs-jni-jvm-clj-eval-direct-test/eval-returns-symbol ()
  "A Clojure symbol result comes back as a real Lisp symbol."
  (jvm-clj-eval-direct-test--ensure-ready)
  (should (eq (jvm-clj-eval-string-direct "'foo") 'foo)))
```

- [ ] **Step 6: Run — confirmation.** With Steps 1-5 applied (impl + tests together), this is the confirmation run:

Run: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-clj-eval-direct-test.el 2>&1 | tail -25`
Expected: `eval-returns-keyword`/`eval-returns-symbol` PASS, existing scalar/ThreadLocal tests PASS. (To see the genuine red first, temporarily revert Step 2's rewire — the tests then fail with an opaque pointer `≠ :a` — then re-apply.)

- [ ] **Step 7: Run both eval files — expect all PASS:**

Run: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-clj-eval-direct-test.el test/jvm-clj-eval-test.el 2>&1 | tail -30`
Expected: all PASS — the new keyword/symbol tests, the scalar `42`, the error `Divide by zero`, the `-await` eval, the nREPL tests.

- [ ] **Step 8: Async parity check** — add to `test/jvm-clj-eval-test.el` before its `(provide ...)`:

```elisp
(ert-deftest emacs-jni-jvm-clj-eval-test/eval-string-await-returns-keyword ()
  (jvm-clj-eval-test--ensure-ready)
  (should (eq (jvm-clj-eval-string-await ":a") :a)))
```

- [ ] **Step 9: Run it — expect PASS** (exercises the async wire path from Task 2):

Run: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-clj-eval-test.el 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 10: Commit:**

```bash
git add clojure/emacs_jni/clj.clj elisp/jvm-clj.el test/jvm-clj-eval-test.el test/jvm-clj-eval-direct-test.el
git commit -m "feat: jvm-clj-eval-string returns Lisp keywords/symbols via emacs-jni.clj"
```

---

## Final verification

- [ ] **Full Rust:** `cargo fmt --check && cargo clippy --all-targets 2>&1 | tail -5 && cargo test 2>&1 | tail -10` → clean, all pass.
- [ ] **Full ERT (symbol-relevant files):** `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-symbol-marshal-test.el test/jvm-clj-eval-test.el test/jvm-clj-eval-direct-test.el test/jvm-callback-wrap-test.el test/jvm-call-direct-test.el` → all pass.
- [ ] **Manual sanity (the reported case):** confirm `(jvm-clj-eval-string ":a")` → `:a` appears in the passing `eval-returns-keyword` test output.
- [ ] Update `docs/superpowers/plans/` checkboxes and note any follow-ups (e.g. caching the `find_class` lookups in `object_to_lisp_arg`).
