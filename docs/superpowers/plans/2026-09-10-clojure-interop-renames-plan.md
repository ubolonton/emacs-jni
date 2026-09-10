# Clojure interop renames + Lisp import + structured errors — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename the JVM/Clojure/Emacs layers to their final names, replace the flat `LispError` with structured `LispSignal`/`LispThrow` at the Clojure layer, and add Lisp-symbol/function import into Clojure vars.

**Architecture:** Three mechanical renames first (each guarded by the existing test suite), then two features. Structured errors: Rust extracts `emacs::ErrorKind::{Signal,Throw}`, marshals the parts through the existing `value_to_reply`, and completes the future exceptionally with a neutral Java carrier `LispNonLocalExit`; `emacs-jni.clj` rethrows it as an `ex-info`. Import: `emacs-jni.clj` returns small `IFn` `deftype`s wrapping either a symbol name marker (late binding) or a resolved function object (early binding).

**Tech Stack:** Rust (`emacs` 0.21, `jni`), Java (JDK 26 via mise), Clojure (loaded from source at `require` time — no AOT), Emacs Lisp (ert).

**Spec:** `docs/superpowers/specs/2026-09-10-clojure-interop-renames-design.md`

## Global Constraints

- Real Java package is `emacs_jni.core` (Java identifiers cannot contain `-`); `emacs-jni.core` is the conceptual name only.
- The Rust module's own feature stays `emacs-jni` (`src/lib.rs` `init` → `(provide 'emacs-jni)`), and the elisp `(featurep 'emacs-jni)` guard around `module-load` stays `emacs-jni`. Only the elisp *file feature* becomes `jvm`. "emacs-jni" is the JVM-side/native name; do not rename the Cargo lib (`emacs_jni`), `libemacs_jni.so`, or the env var `EMACS_JNI_MODULE_PATH`.
- Clojure loads from source at `require` time — no `gen-class`/AOT. Error types are `ex-info`, not concrete classes.
- Writing style for prose (comments, commit messages, docs): ASD-STE100 Simplified Technical English, minimal words, engineer-to-engineer.
- Build: `cargo build` (runs `build.rs`, which compiles the Java shim). Rust tests: `cargo test`. Emacs tests: `./test/run-ert-tests.sh [file...]` (one `emacs --batch` per file; builds first).

---

### Task 1: Rename Java package `emacs_jni` → `emacs_jni.core`

**Files:**
- Move: `java/emacs_jni/*.java` → `java/emacs_jni/core/*.java` (8 files: `Lisp`, `LispCallable`, `LispFunction`, `LispError`, `LispObject`, `LispRefReleaseNotifier`, `LispSymbol`, `ReleaseNotifier`)
- Modify: `build.rs` (paths + `rerun-if-changed`), `src/shim.rs` (every `emacs_jni/X` class path, `include_bytes!` paths, doc comments), `src/jvm_thread.rs` (every `emacs_jni/X`), `src/marshal.rs` (every `emacs_jni/X`), `src/commands.rs` + `src/lisp_requests.rs` (doc comments naming `emacs_jni.X`)
- Modify: `test/fixtures/Calc.java` (`emacs_jni.LispCallable` → `emacs_jni.core.LispCallable`)
- Modify: `test/jvm-clj-test.el` (`emacs_jni.LispObject`/`emacs_jni.LispError` → `emacs_jni.core.*`)

**Interfaces:**
- Produces: Java package `emacs_jni.core`; all shim classes now at `emacs_jni/core/<Name>`.

- [ ] **Step 1: Move the Java files and rewrite their `package` line**

```bash
mkdir -p java/emacs_jni/core
git mv java/emacs_jni/Lisp.java java/emacs_jni/core/Lisp.java
git mv java/emacs_jni/LispCallable.java java/emacs_jni/core/LispCallable.java
git mv java/emacs_jni/LispFunction.java java/emacs_jni/core/LispFunction.java
git mv java/emacs_jni/LispError.java java/emacs_jni/core/LispError.java
git mv java/emacs_jni/LispObject.java java/emacs_jni/core/LispObject.java
git mv java/emacs_jni/LispRefReleaseNotifier.java java/emacs_jni/core/LispRefReleaseNotifier.java
git mv java/emacs_jni/LispSymbol.java java/emacs_jni/core/LispSymbol.java
git mv java/emacs_jni/ReleaseNotifier.java java/emacs_jni/core/ReleaseNotifier.java
```

In each moved file change the first line `package emacs_jni;` → `package emacs_jni.core;`.

- [ ] **Step 2: Update `build.rs`**

Rewrite every `java/emacs_jni/<Name>.java` path to `java/emacs_jni/core/<Name>.java`, in both the `rerun-if-changed` lines and the `javac` args. The `-d` output dir (`target/emacs-jni-shim-classes`) is unchanged — `javac` writes to `emacs_jni/core/` under it from the `package` line. Update the final `assert!` message text if it names the package.

- [ ] **Step 3: Update Rust class paths**

In `src/shim.rs`, `src/jvm_thread.rs`, `src/marshal.rs`: replace every `"emacs_jni/<Name>"` with `"emacs_jni/core/<Name>"` (in `find_class`, `define_if_absent`, and the `include_bytes!` paths `.../emacs-jni-shim-classes/emacs_jni/<Name>.class` → `.../emacs_jni/core/<Name>.class`). Update doc comments that name `emacs_jni.<Name>` to `emacs_jni.core.<Name>`. In `src/commands.rs` and `src/lisp_requests.rs` update the doc comments that mention `emacs_jni.LispSymbol` to `emacs_jni.core.LispSymbol`. Do not touch the `dev.ublt.emacs.clj` mentions yet (Task 3).

- [ ] **Step 4: Update the Java test fixture and the one ert reference**

In `test/fixtures/Calc.java`: `emacs_jni.LispCallable` → `emacs_jni.core.LispCallable` (both the `instanceof` and the two method parameter types).
In `test/jvm-clj-test.el`: `emacs_jni.LispObject` → `emacs_jni.core.LispObject` and `emacs_jni.LispError` → `emacs_jni.core.LispError` (the error test is rewritten to `ex-info` in Task 4; keep it compiling here).

- [ ] **Step 5: Build and run the Rust tests**

Run: `cargo build && cargo test`
Expected: PASS (shim compiles at the new package path; no behavior change).

- [ ] **Step 6: Run the ert suite**

Run: `./test/run-ert-tests.sh test/jvm-clj-test.el test/jvm-call-test.el test/jvm-invoke-test.el`
Expected: PASS. (These exercise the shim classes, the callback path, and the Clojure marshaling that names `LispObject`.)

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "Rename Java package emacs_jni -> emacs_jni.core"
```

---

### Task 2: Rename Emacs file `emacs-jni.el` → `jvm.el` (feature `jvm`)

**Files:**
- Move: `elisp/emacs-jni.el` → `elisp/jvm.el`
- Modify: `elisp/jvm.el` (header, `defvar` name, final `provide`)
- Modify: `elisp/jvm-clj.el` (`(require 'emacs-jni)` → `(require 'jvm)`)
- Modify: `test/*.el` that `(require 'emacs-jni)` (`jvm-handle-test`, `jvm-call-test`, `jvm-invoke-test`, `jvm-pause-test`, `jvm-clj-test`, `smoke-test`, and any other match)

**Interfaces:**
- Produces: elisp feature `jvm` from `elisp/jvm.el`. The native module feature stays `emacs-jni`.

- [ ] **Step 1: Move the file**

```bash
git mv elisp/emacs-jni.el elisp/jvm.el
```

- [ ] **Step 2: Edit `elisp/jvm.el`**

- Header line: `;;; emacs-jni.el --- JVM embedding for Emacs ...` → `;;; jvm.el --- JVM embedding for Emacs ...`.
- Rename the var `emacs-jni--module-path` → `jvm--module-path` (both the `defvar` and its use in the `module-load` call).
- Leave the `module-load` guard as `(unless (featurep 'emacs-jni) ...)` — that guards the native module, whose feature is `emacs-jni` (see Global Constraints). Do NOT change it to `jvm`.
- Final line `(provide 'emacs-jni)` → `(provide 'jvm)`.
- Leave `EMACS_JNI_MODULE_PATH` and the `libemacs_jni.so` filename unchanged.

- [ ] **Step 3: Update requirers**

In `elisp/jvm-clj.el`: `(require 'emacs-jni)` → `(require 'jvm)`.
In every `test/*.el` matching `(require 'emacs-jni)`: change to `(require 'jvm)`.

Verify none remain: `grep -rn "require 'emacs-jni\|provide 'emacs-jni\|emacs-jni--module-path" elisp/ test/` — the only surviving `emacs-jni` reference in elisp must be the `(featurep 'emacs-jni)` guard in `jvm.el`.

- [ ] **Step 4: Run the ert suite**

Run: `./test/run-ert-tests.sh`
Expected: PASS across all files (each loads `-L elisp` and requires `jvm`/`jvm-clj`). `jvm-crash-test.el` is auto-skipped without `test-support`.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "Rename elisp emacs-jni.el -> jvm.el (feature jvm)"
```

---

### Task 3: Rename Clojure namespace `dev.ublt.emacs.clj` → `emacs-jni.clj`

**Files:**
- Move: `clojure-src/dev/ublt/emacs/clj.clj` → `clojure-src/emacs_jni/clj.clj`
- Modify: `clojure-src/emacs_jni/clj.clj` (`ns` form, `:import` package)
- Modify: `elisp/jvm-clj.el` (require string `"dev.ublt.emacs.clj"` → `"emacs-jni.clj"`, and the two error-message/doc mentions)
- Modify: `test/jvm-clj-test.el`, `test/jvm-clojure-test.el` (every `dev.ublt.emacs.clj` → `emacs-jni.clj`)
- Modify: `src/lisp_requests.rs`, `src/commands.rs` (doc comments mentioning `dev.ublt.emacs.clj`)

**Interfaces:**
- Produces: Clojure namespace `emacs-jni.clj` (file `clojure-src/emacs_jni/clj.clj`).

- [ ] **Step 1: Move the source file to the munged path**

```bash
mkdir -p clojure-src/emacs_jni
git mv clojure-src/dev/ublt/emacs/clj.clj clojure-src/emacs_jni/clj.clj
rmdir -p clojure-src/dev/ublt/emacs 2>/dev/null || true
```

- [ ] **Step 2: Edit the `ns` form and imports**

In `clojure-src/emacs_jni/clj.clj`: `(ns dev.ublt.emacs.clj ...)` → `(ns emacs-jni.clj ...)`. Update the docstring's spec path reference if present. Change `(:import [emacs_jni Lisp LispObject LispSymbol] ...)` → `(:import [emacs_jni.core Lisp LispObject LispSymbol] ...)`.

- [ ] **Step 3: Update elisp and tests**

In `elisp/jvm-clj.el`: the `jvm-clojure-require` argument `"dev.ublt.emacs.clj"` → `"emacs-jni.clj"`; update the two doc/error strings that name the namespace.
In `test/jvm-clj-test.el` and `test/jvm-clojure-test.el`: replace every `dev.ublt.emacs.clj` with `emacs-jni.clj` (docstrings, comments, and the many `(dev.ublt.emacs.clj/…)` eval strings).

Verify: `grep -rn "dev\.ublt\|dev/ublt" clojure-src/ elisp/ test/ src/` returns nothing.

- [ ] **Step 4: Update Rust doc comments**

In `src/lisp_requests.rs` and `src/commands.rs`, change `dev.ublt.emacs.clj` mentions to `emacs-jni.clj`. (Comments only — no code path uses the string.)

- [ ] **Step 5: Build and test**

Run: `cargo build && ./test/run-ert-tests.sh test/jvm-clj-test.el test/jvm-clojure-test.el`
Expected: PASS (the namespace requires and evaluates under its new name).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "Rename Clojure namespace dev.ublt.emacs.clj -> emacs-jni.clj"
```

---

### Task 4: Structured errors — `LispSignal` / `LispThrow`

**Files:**
- Create: `java/emacs_jni/core/LispNonLocalExit.java`
- Delete: `java/emacs_jni/core/LispError.java`
- Modify: `build.rs` (swap `LispError.java` for `LispNonLocalExit.java` in both lists)
- Modify: `src/shim.rs` (class-const `include_bytes!`, `define_if_absent`, `find_class`: drop `LispError`, add `LispNonLocalExit`)
- Modify: `src/commands.rs` (`LispReplyValue`: replace `Error(String)` with `Signal`/`Throw`)
- Modify: `src/lisp_requests.rs` (`reply` takes `&Env`; add `error_to_reply`; update every `reply(id, …)` call site to `reply(env, id, …)`; add a unit test)
- Modify: `src/jvm_thread.rs` (`handle_lisp_reply`, `handle_invoke_reply`, `build_lisp_reply_object`)
- Modify: `clojure-src/emacs_jni/clj.clj` (import `LispNonLocalExit`; `apply`/`apply-async` translate to `ex-info`; add predicates)
- Modify: `test/jvm-clj-test.el` (rewrite the error test to assert `ex-info`; add a throw test)

**Interfaces:**
- Consumes: `value_to_reply(env, value) -> EmacsResult<LispReplyValue>`, `build_lisp_reply_object(env, LispReplyValue) -> JObject` (from earlier code).
- Produces:
  - Rust `LispReplyValue::Signal { symbol: Box<LispReplyValue>, data: Box<LispReplyValue> }` and `Throw { tag: Box<LispReplyValue>, value: Box<LispReplyValue> }`.
  - Java `emacs_jni.core.LispNonLocalExit(int kind, Object symbolOrTag, Object dataOrValue)` with `public static final int SIGNAL = 0, THROW = 1;` and public final fields `kind`, `symbolOrTag`, `dataOrValue`.
  - Clojure: `apply`/`apply-async` throw `ex-info` with ex-data `{:type ::lisp-signal :symbol s :data d}` or `{:type ::lisp-throw :tag t :value v}`; predicates `lisp-signal?`, `lisp-throw?`.

- [ ] **Step 1: Write the failing ert tests**

In `test/jvm-clj-test.el`, replace the existing `LispError` catch test and add a throw test. Use the file's existing `jvm-clj-test--eval` helper (evaluates a Clojure string and returns the Lisp value).

```elisp
(ert-deftest jvm-clj-test-apply-error-becomes-lisp-signal ()
  "A Lisp `error' surfaces to Clojure as an ex-info tagged ::lisp-signal,
carrying the signal symbol and data."
  (should
   (equal
    (jvm-clj-test--eval
     "(try (emacs-jni.clj/apply (emacs-jni.clj/intern \"error\") \"boom\")
        (catch clojure.lang.ExceptionInfo e
          (let [d (ex-data e)]
            (str (:type d) \"|\" (:symbol d)))))")
    ":emacs-jni.clj/lisp-signal|error")))

(ert-deftest jvm-clj-test-throw-becomes-lisp-throw ()
  "A Lisp `throw' surfaces to Clojure as an ex-info tagged ::lisp-throw,
carrying tag and value."
  (should
   (equal
    (jvm-clj-test--eval
     "(let [f (emacs-jni.clj/intern \"eval\")
            form (emacs-jni.clj/apply (emacs-jni.clj/intern \"read\")
                                       \"(throw 'my-tag 42)\")]
        (try (emacs-jni.clj/apply f form)
          (catch clojure.lang.ExceptionInfo e
            (let [d (ex-data e)]
              (str (:type d) \"|\" (:tag d) \"|\" (:value d))))))")
    ":emacs-jni.clj/lisp-throw|my-tag|42")))
```

- [ ] **Step 2: Run them to confirm they fail**

Run: `./test/run-ert-tests.sh test/jvm-clj-test.el`
Expected: FAIL (current code throws `LispError`, not an `ex-info`; `apply` does not translate).

- [ ] **Step 3: Add the Java carrier, remove `LispError`**

Create `java/emacs_jni/core/LispNonLocalExit.java`:

```java
package emacs_jni.core;

// Neutral carrier for a Lisp non-local exit crossing to Clojure. Built by
// Rust (jvm_thread), completed onto the reply future via
// completeExceptionally. The emacs-jni.clj namespace rethrows it as an
// ex-info; this class stays a plain platform-loader RuntimeException so
// Rust never needs to reach the Clojure classloader. `symbolOrTag` and
// `dataOrValue` are already marshaled (a LispSymbol marker, a LispObject,
// or a boxed scalar).
public final class LispNonLocalExit extends RuntimeException {
    public static final int SIGNAL = 0;
    public static final int THROW = 1;

    public final int kind;
    public final Object symbolOrTag;
    public final Object dataOrValue;

    public LispNonLocalExit(int kind, Object symbolOrTag, Object dataOrValue) {
        super();
        this.kind = kind;
        this.symbolOrTag = symbolOrTag;
        this.dataOrValue = dataOrValue;
    }
}
```

```bash
git rm java/emacs_jni/core/LispError.java
```

In `build.rs`: in both the `rerun-if-changed` list and the `javac` arg list, replace the `LispError.java` entry with `LispNonLocalExit.java`.

In `src/shim.rs`: replace the `LispError` class constant (`include_bytes!` of `emacs_jni/core/LispError.class`) with a `LispNonLocalExit` one (`.../emacs_jni/core/LispNonLocalExit.class`); replace the `define_if_absent(env, &platform_loader, "emacs_jni/core/LispError", …)` line with `"emacs_jni/core/LispNonLocalExit"`; replace the `find_class("emacs_jni/core/LispError")` verify line with `"emacs_jni/core/LispNonLocalExit"`. Update the module doc comment that lists the shim classes.

- [ ] **Step 4: Replace the Rust `LispReplyValue::Error` variant**

In `src/commands.rs`, remove `Error(String)` and add:

```rust
    /// A Lisp `signal` — the future completes exceptionally with a
    /// `LispNonLocalExit(SIGNAL, symbol, data)`. `symbol` and `data` are
    /// nested, already-marshaled reply values (a symbol marker; the data
    /// list as a `LispObject`).
    Signal { symbol: Box<LispReplyValue>, data: Box<LispReplyValue> },
    /// A Lisp `throw` — `LispNonLocalExit(THROW, tag, value)`.
    Throw { tag: Box<LispReplyValue>, value: Box<LispReplyValue> },
```

Update the `Symbol` variant doc comment already fixed in Task 3 if needed (no change).

- [ ] **Step 5: Marshal the error in `lisp_requests.rs`**

Change `reply` to take `&Env` and delegate error conversion to a new `error_to_reply`:

```rust
/// Sends a `Cmd::LispReply` for ID. An `Err` is converted to a `Signal`
/// or `Throw` reply via `error_to_reply` (marshaling the signal symbol/
/// data or throw tag/value through `value_to_reply`).
fn reply(env: &Env, id: u64, result: EmacsResult<LispReplyValue>) {
    let value = match result {
        Ok(v) => v,
        Err(e) => error_to_reply(env, e),
    };
    let _ = worker::send_command(Cmd::LispReply { id, value });
}

/// Converts an `emacs::Error` into a `LispReplyValue::{Signal,Throw}`. A
/// `Signal`/`Throw` `ErrorKind` carries the real symbol/data or tag/value
/// (each a `TempValue` bound with this `Env` -- sound here because `reply`
/// runs on the main thread with the `Env` the failing call used). Anything
/// else (a Rust-internal error, or a marshaling failure while building the
/// nested values) collapses to a `Signal` whose symbol is `error` and
/// whose data is the formatted message string.
fn error_to_reply(env: &Env, e: emacs::Error) -> LispReplyValue {
    use emacs::ErrorKind;
    let boxed = |r: EmacsResult<LispReplyValue>| r.ok().map(Box::new);
    match e.downcast_ref::<ErrorKind>() {
        Some(ErrorKind::Signal { symbol, data }) => {
            let s = boxed(value_to_reply(env, unsafe { symbol.value(env) }));
            let d = boxed(value_to_reply(env, unsafe { data.value(env) }));
            match (s, d) {
                (Some(symbol), Some(data)) => LispReplyValue::Signal { symbol, data },
                _ => fallback_signal(&format!("{e}")),
            }
        }
        Some(ErrorKind::Throw { tag, value }) => {
            let t = boxed(value_to_reply(env, unsafe { tag.value(env) }));
            let v = boxed(value_to_reply(env, unsafe { value.value(env) }));
            match (t, v) {
                (Some(tag), Some(value)) => LispReplyValue::Throw { tag, value },
                _ => fallback_signal(&format!("{e}")),
            }
        }
        _ => fallback_signal(&format!("{e}")),
    }
}

/// A `Signal` reply for a non-`ErrorKind` error: symbol `error`, data the
/// message string. Keeps every failure arriving at Clojure as a signal.
fn fallback_signal(message: &str) -> LispReplyValue {
    LispReplyValue::Signal {
        symbol: Box::new(LispReplyValue::Symbol { name: "error".to_string(), keyword: false }),
        data: Box::new(LispReplyValue::Str(message.to_string())),
    }
}
```

Then update every `reply(id, …)` call in `service` (there are 7: `Intern`, `Apply`, `TypeOf`, `IsNotNil`, `Eq`, `Prin1`) to `reply(env, id, …)`.

- [ ] **Step 6: Build the carrier and complete the future in `jvm_thread.rs`**

In `handle_lisp_reply`, replace the `if let LispReplyValue::Error(message) = value` arm with a match that intercepts `Signal`/`Throw` before `build_lisp_reply_object`:

```rust
match value {
    crate::commands::LispReplyValue::Signal { symbol, data } => {
        complete_with_non_local_exit(env, &future_local, 0, *symbol, *data)?;
    }
    crate::commands::LispReplyValue::Throw { tag, value } => {
        complete_with_non_local_exit(env, &future_local, 1, *tag, *value)?;
    }
    other => {
        let obj = build_lisp_reply_object(env, other)?;
        env.call_method(
            &future_local,
            jni::jni_str!("complete"),
            jni::jni_sig!((v: java.lang.Object) -> boolean),
            &[jni::objects::JValueOwned::Object(obj).borrow()],
        )?;
    }
}
```

Add the helper (near `build_lisp_reply_object`):

```rust
/// Builds an `emacs_jni.core.LispNonLocalExit(kind, symbolOrTag,
/// dataOrValue)` from two already-classified reply values and completes
/// FUTURE exceptionally with it. `kind` is 0 (signal) or 1 (throw).
fn complete_with_non_local_exit<'local>(
    env: &mut jni::Env<'local>,
    future: &jni::objects::JObject<'local>,
    kind: i32,
    a: crate::commands::LispReplyValue,
    b: crate::commands::LispReplyValue,
) -> jni::errors::Result<()> {
    let a_obj = build_lisp_reply_object(env, a)?;
    let b_obj = build_lisp_reply_object(env, b)?;
    let class = env.find_class(jni::jni_str!("emacs_jni/core/LispNonLocalExit"))?;
    let ex = env.new_object(
        &class,
        jni::jni_sig!((kind: int, a: java.lang.Object, b: java.lang.Object) -> void),
        &[
            jni::objects::JValue::Int(kind),
            jni::objects::JValue::Object(&a_obj),
            jni::objects::JValue::Object(&b_obj),
        ],
    )?;
    env.call_method(
        future,
        jni::jni_str!("completeExceptionally"),
        jni::jni_sig!((ex: java.lang.Throwable) -> boolean),
        &[jni::objects::JValueOwned::Object(ex).borrow()],
    )?;
    Ok(())
}
```

In `build_lisp_reply_object`, replace the `LispReplyValue::Error(_) => unreachable!(...)` arm with two arms:

```rust
        crate::commands::LispReplyValue::Signal { .. } | crate::commands::LispReplyValue::Throw { .. } => {
            unreachable!("Signal/Throw are handled by handle_lisp_reply before build_lisp_reply_object")
        }
```

In `handle_invoke_reply` (the failed-callback path), replace the `LispError` construction with a `LispNonLocalExit` signal carrying the message. Reuse `complete_with_non_local_exit`:

```rust
            complete_with_non_local_exit(
                env,
                &future_local,
                0,
                crate::commands::LispReplyValue::Symbol { name: "error".to_string(), keyword: false },
                crate::commands::LispReplyValue::Str(message),
            )?;
```

Update the doc comments in `jvm_thread.rs`/`shim.rs` that say "LispError" to name `LispNonLocalExit`.

- [ ] **Step 7: Add a Rust unit test for the fallback shape**

In `src/lisp_requests.rs` `mod tests`, add (this covers the `Env`-free path — the live signal/throw marshaling is covered by the ert tests, since it needs a real Emacs `Env`):

```rust
#[test]
fn fallback_signal_is_error_symbol_with_message() {
    match fallback_signal("boom") {
        LispReplyValue::Signal { symbol, data } => {
            assert!(matches!(*symbol, LispReplyValue::Symbol { ref name, keyword: false } if name == "error"));
            assert!(matches!(*data, LispReplyValue::Str(ref s) if s == "boom"));
        }
        _ => panic!("expected Signal"),
    }
}
```

- [ ] **Step 8: Translate to `ex-info` in `emacs-jni.clj`**

In `clojure-src/emacs_jni/clj.clj`, add `LispNonLocalExit` to the `:import` (`[emacs_jni.core Lisp LispObject LispSymbol LispNonLocalExit]`) and replace the `apply` / `apply-async` definitions:

```clojure
(defn- lisp-exit->ex-info
  "Translate a native LispNonLocalExit carrier into a Clojure ex-info,
  converting its parts with <-lisp."
  [^LispNonLocalExit e]
  (let [a (<-lisp (.-symbolOrTag e))
        b (<-lisp (.-dataOrValue e))]
    (if (= (.-kind e) LispNonLocalExit/SIGNAL)
      (ex-info (str "Lisp signal: " a) {:type ::lisp-signal :symbol a :data b} e)
      (ex-info (str "Lisp throw: " a) {:type ::lisp-throw :tag a :value b} e))))

(defn- throw-lisp-exit
  "Given a completion throwable (a bare LispNonLocalExit from .handle, or
  one wrapped in an ExecutionException from deref), rethrow it as an
  ex-info. Anything else is rethrown unchanged."
  [t]
  (let [cause (if (instance? java.util.concurrent.ExecutionException t) (.getCause t) t)]
    (if (instance? LispNonLocalExit cause)
      (throw (lisp-exit->ex-info cause))
      (throw t))))

(defn apply-async
  "Apply F to ARGS asynchronously. Returns a CompletableFuture of the
  converted result. A Lisp non-local exit surfaces as an ex-info
  (::lisp-signal or ::lisp-throw)."
  [f & args]
  (.handle (apply* f args)
    (reify java.util.function.BiFunction
      (apply [_ v t] (if t (throw-lisp-exit t) (<-lisp v))))))

(defn apply
  "Apply F to ARGS, blocking for the result. A Lisp non-local exit surfaces
  as an ex-info (::lisp-signal or ::lisp-throw)."
  [f & args]
  (try
    (<-lisp (deref (apply* f args)))
    (catch Throwable t (throw-lisp-exit t))))

(defn lisp-signal?
  "True if E is a Lisp-signal ex-info (from `apply`)."
  [e]
  (= (:type (ex-data e)) ::lisp-signal))

(defn lisp-throw?
  "True if E is a Lisp-throw ex-info (from `apply`)."
  [e]
  (= (:type (ex-data e)) ::lisp-throw))
```

Add `[java.util.function BiFunction Function]` (or fully-qualify `BiFunction` as above) — `apply-async` already imported `Function` for the old `thenApply`; the new form uses `BiFunction` on `.handle`, so drop the now-unused `Function` import if nothing else uses it.

- [ ] **Step 9: Run the ert tests and Rust tests**

Run: `cargo test && ./test/run-ert-tests.sh test/jvm-clj-test.el`
Expected: PASS — both new error tests, plus the unchanged marshaling tests in the file.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "Replace LispError with structured LispSignal/LispThrow via ex-info"
```

---

### Task 5: Import Lisp symbols and functions into Clojure

**Files:**
- Modify: `clojure-src/emacs_jni/clj.clj` (helpers, callable `deftype`s, `require-symbol`/`refer-symbols`/`require-function`/`refer-functions`)
- Modify: `test/jvm-clj-test.el` (import tests)

**Interfaces:**
- Consumes: `apply*` (native `applyAsync` with marshaled args), `apply` (blocking Lisp apply), `<-lisp`, the `LispSymbol` marker class.
- Produces:
  - `(require-symbol sym) -> LispSymbolFn` (late binding; `sym` a Clojure symbol/keyword/string naming a Lisp symbol).
  - `(require-function sym) -> LispFunctionFn` (early binding; resolves `indirect-function` now).
  - Macros `(refer-symbols & syms)`, `(refer-functions & syms)` that `def` a var per symbol in the calling namespace.
  - `LispSymbolFn`/`LispFunctionFn` are `clojure.lang.IFn` `deftype`s in `emacs-jni.clj` wrapping a receiver.

- [ ] **Step 1: Write the failing ert tests**

In `test/jvm-clj-test.el`:

```elisp
(ert-deftest jvm-clj-test-require-symbol-is-callable ()
  "require-symbol returns an IFn that funcalls the Lisp symbol."
  (should
   (= (jvm-clj-test--eval
       "((emacs-jni.clj/require-symbol 'concat) \"a\" \"b\")")
      "ab"))
  (should
   (= (jvm-clj-test--eval
       "(clojure.core/apply (emacs-jni.clj/require-symbol '+) [1 2 3])")
      6)))

(ert-deftest jvm-clj-test-refer-symbols-defs-vars ()
  "refer-symbols creates callable vars in the current namespace."
  (should
   (= (jvm-clj-test--eval
       "(do (emacs-jni.clj/refer-symbols concat) (concat \"x\" \"y\"))")
      "xy")))

(ert-deftest jvm-clj-test-require-function-is-early-bound ()
  "require-function captures the function now; a later fset of the symbol
does not change the imported function, while require-symbol tracks it."
  (should
   (equal
    (jvm-clj-test--eval
     "(do (emacs-jni.clj/apply (emacs-jni.clj/intern \"defalias\")
            (emacs-jni.clj/intern \"jvm-clj-test-fn\") (emacs-jni.clj/intern \"car\"))
          (let [early (emacs-jni.clj/require-function 'jvm-clj-test-fn)
                late  (emacs-jni.clj/require-symbol 'jvm-clj-test-fn)]
            (emacs-jni.clj/apply (emacs-jni.clj/intern \"defalias\")
              (emacs-jni.clj/intern \"jvm-clj-test-fn\") (emacs-jni.clj/intern \"cdr\"))
            (let [lst (emacs-jni.clj/apply (emacs-jni.clj/intern \"list\") 1 2 3)]
              [(early lst) (str (late lst))])))")
    ;; early still `car' -> 1; late now `cdr' -> opaque LispObject (2 3),
    ;; whose prin1 is "(2 3)".
    [1 "(2 3)"])))
```

- [ ] **Step 2: Run to confirm failure**

Run: `./test/run-ert-tests.sh test/jvm-clj-test.el`
Expected: FAIL (`require-symbol`/`require-function`/`refer-symbols` undefined).

- [ ] **Step 3: Add the name helper and the Lisp-apply runner**

In `clojure-src/emacs_jni/clj.clj` (after `apply`):

```clojure
(defn- sym->lisp-name
  "The Lisp symbol name for SYM (a Clojure symbol, keyword, or string),
  preserving a namespace segment as a `foo/bar` Lisp name."
  [sym]
  (if-let [ns (and (or (symbol? sym) (keyword? sym)) (namespace sym))]
    (str ns "/" (name sym))
    (name sym)))

(defn- run-lisp
  "Apply RECEIVER (a LispSymbol marker or a function LispObject) to ARGS
  (a seq), blocking for the converted result; a non-local exit surfaces as
  an ex-info, same as `apply`."
  [receiver args]
  (try
    (<-lisp (deref (apply* receiver args)))
    (catch Throwable t (throw-lisp-exit t))))
```

- [ ] **Step 4: Add the callable `deftype`s**

```clojure
(defmacro ^:private def-lisp-callable
  "Define a deftype NAME wrapping a `receiver`, applying it to its call
  args through `run-lisp`. Covers direct arities 0-5 plus applyTo (for
  `clojure.core/apply` and higher arities)."
  [name]
  `(deftype ~name [~'receiver]
     clojure.lang.IFn
     (invoke [_#] (run-lisp ~'receiver []))
     (invoke [_# a#] (run-lisp ~'receiver [a#]))
     (invoke [_# a# b#] (run-lisp ~'receiver [a# b#]))
     (invoke [_# a# b# c#] (run-lisp ~'receiver [a# b# c#]))
     (invoke [_# a# b# c# d#] (run-lisp ~'receiver [a# b# c# d#]))
     (invoke [_# a# b# c# d# e#] (run-lisp ~'receiver [a# b# c# d# e#]))
     (applyTo [_# args#] (run-lisp ~'receiver (seq args#)))))

;; Late binding: wraps a symbol-name marker; each call re-funcalls the
;; symbol, so a later redefinition is picked up.
(def-lisp-callable LispSymbolFn)

;; Early binding: wraps the resolved function object; each call applies
;; that same object.
(def-lisp-callable LispFunctionFn)
```

- [ ] **Step 5: Add the four public entry points**

```clojure
(defn require-symbol
  "Import Lisp symbol SYM as a callable value (late binding, like
  `emacs::use_symbols!`). Returns a LispSymbolFn: call it to funcall SYM."
  [sym]
  (->LispSymbolFn (LispSymbol. (sym->lisp-name sym) false)))

(defmacro refer-symbols
  "Define a var per SYM in the current namespace, each the LispSymbolFn
  from `require-symbol`. So `(refer-symbols message)` makes
  `(message \"hi\")` call Lisp `message`."
  [& syms]
  `(do ~@(map (fn [s] `(def ~s (require-symbol '~s))) syms)))

(defn require-function
  "Import Lisp function SYM as a callable value (early binding, like
  `emacs::use_functions!`). Resolves `indirect-function` now and holds the
  function object; a later redefinition of SYM does not affect it."
  [sym]
  (let [f (apply (LispSymbol. "indirect-function" false)
                 (LispSymbol. (sym->lisp-name sym) false))]
    (->LispFunctionFn f)))

(defmacro refer-functions
  "Define a var per SYM in the current namespace, each the LispFunctionFn
  from `require-function`."
  [& syms]
  `(do ~@(map (fn [s] `(def ~s (require-function '~s))) syms)))
```

Note for the implementer: `refer-symbols`/`refer-functions` work only for Lisp names that are also legal Clojure `def` symbols. For a Lisp name that is not (e.g. `1+`), use `require-symbol`/`require-function` with a string and bind it yourself.

- [ ] **Step 6: Run the ert tests**

Run: `./test/run-ert-tests.sh test/jvm-clj-test.el`
Expected: PASS (all import tests, error tests, and existing marshaling tests).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "Add Lisp symbol/function import into Clojure (require/refer)"
```

---

### Task 6: Update documentation

**Files:**
- Modify: `docs/how-to-guides/access-lisp-from-clojure.md`, `docs/reference/lisp-api.md`, `docs/explanation/interop-design.md`
- Modify: any other doc/README hit by the name sweep

**Interfaces:**
- Consumes: the final names and APIs from Tasks 1-5.

- [ ] **Step 1: Sweep for stale names**

Run: `grep -rln "dev\.ublt\|emacs_jni\.Lisp\|emacs-jni\.el\|LispError\|(require 'emacs-jni" docs/ README.md`
List every file. (Ignore `docs/superpowers/specs/` and `docs/superpowers/plans/` — those are historical records, except this task's own spec/plan which are already current.)

- [ ] **Step 2: Update the three primary docs**

In `docs/how-to-guides/access-lisp-from-clojure.md`, `docs/reference/lisp-api.md`, `docs/explanation/interop-design.md`: replace `dev.ublt.emacs.clj` → `emacs-jni.clj`, `emacs_jni.X` → `emacs_jni.core.X`, `emacs-jni.el` → `jvm.el`, and the `LispError` error section → the `ex-info` (`::lisp-signal`/`::lisp-throw`) contract. Keep ASD-STE100 style.

- [ ] **Step 3: Document the import API and error handling**

In `docs/how-to-guides/access-lisp-from-clojure.md` add a section covering `require-symbol`/`refer-symbols` (late) and `require-function`/`refer-functions` (early), with the `((require-symbol 'message) "foo")` and `(refer-symbols message)` examples, and a short `try`/`catch clojure.lang.ExceptionInfo` example reading `(:symbol (ex-data e))` for a `::lisp-signal`. In `docs/reference/lisp-api.md` add these four to the API reference.

- [ ] **Step 4: Verify no stale names remain**

Run: `grep -rln "dev\.ublt\|emacs_jni\.Lisp[^.]\|LispError\|(require 'emacs-jni" docs/ README.md`
Expected: no hits outside `docs/superpowers/` history.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "Update docs for renames, structured errors, and Lisp import"
```

---

## Self-Review

**Spec coverage:**
- §1 Renames → Tasks 1 (Java pkg), 2 (Emacs file/feature), 3 (Clojure ns). Cargo lib / `.so` / env var untouched (Global Constraints). ✓
- §2 Structured errors (Rust variants, Java carrier, Clojure ex-info, fallback confirmed, `indirect`-independent) → Task 4. ✓
- §3 Import symbol/function (late/early, `indirect-function`, IFn wrappers, refer macros) → Task 5. ✓
- §4 Testing (Rust unit + ert for signal/throw/import) → Tasks 4-5; docs → Task 6. ✓

**Placeholder scan:** No TBD/TODO; every code step carries real code. The `preamble` namespace is explicitly out of scope (spec Non-goals). ✓

**Type consistency:** `LispNonLocalExit(int, Object, Object)` with fields `kind`/`symbolOrTag`/`dataOrValue` and constants `SIGNAL`/`THROW` are used identically in `jvm_thread.rs` (`complete_with_non_local_exit`) and `clj.clj` (`lisp-exit->ex-info`). `LispReplyValue::{Signal,Throw}` field names (`symbol`/`data`, `tag`/`value`) match between `commands.rs`, `lisp_requests.rs`, and `jvm_thread.rs`. `run-lisp`/`apply*`/`throw-lisp-exit`/`sym->lisp-name` are defined before use. `->LispSymbolFn`/`->LispFunctionFn` come from `def-lisp-callable`. ✓
