# Clojure interop renames + Lisp-symbol/function import + structured errors

Date: 2026-09-10

## Goal

Three coupled changes to the Clojure/JVM/Emacs layers:

1. Rename packages, namespaces, and the Emacs feature file to their final
   names.
2. Replace the flat `LispError` (a formatted message string) with
   structured `LispSignal` (symbol + data) and `LispThrow` (tag + value)
   at the Clojure layer.
3. Add a way to import a Lisp symbol or function as a Clojure callable var,
   the equivalent of `emacs::use_symbols!` (late binding) and
   `emacs::use_functions!` (early binding).

## Non-goals

- No `preamble` (or other) namespace of pre-imported symbols. That is an
  application-side use of the import mechanism, not part of it.
- No change to the Lisp-to-Java callback direction beyond what the error
  rename forces.
- No change to the Cargo lib name (`emacs_jni`) or the native artifact
  (`libemacs_jni.so`). "emacs-jni" stays the JVM-side / native name.

## 1. Renames

| From | To |
| --- | --- |
| Java package `emacs_jni`, dir `java/emacs_jni/` | `emacs_jni.core`, dir `java/emacs_jni/core/` |
| Rust `find_class` / `define_if_absent` / `build.rs` paths `emacs_jni/X` | `emacs_jni/core/X` |
| Clojure ns `dev.ublt.emacs.clj`, file `clojure-src/dev/ublt/emacs/clj.clj` | `emacs-jni.clj`, file `clojure-src/emacs_jni/clj.clj` |
| Emacs file `elisp/emacs-jni.el` | `elisp/jvm.el` |
| `(provide 'emacs-jni)`, `(featurep 'emacs-jni)` | `(provide 'jvm)`, `(featurep 'jvm)` |
| var `emacs-jni--module-path` | `jvm--module-path` |
| `elisp/jvm-clj.el`: `(require 'emacs-jni)`, require string `"dev.ublt.emacs.clj"` | `(require 'jvm)`, `"emacs-jni.clj"` |

Notes:

- `emacs-jni.core` is the conceptual name. The real Java package is
  `emacs_jni.core` (Java identifiers cannot contain `-`).
- The Java package rename touches every `emacs_jni/X` class path in
  `src/shim.rs`, `src/jvm_thread.rs`, `src/marshal.rs`, and the `javac`
  invocation and `rerun-if-changed` lines in `build.rs`, plus doc
  comments naming the classes.
- Clojure-specific classes (the import wrappers, §3) live in the
  `emacs-jni.clj` namespace, not `emacs_jni.core`.
- Env var `EMACS_JNI_MODULE_PATH` is unchanged — it names the native
  artifact, whose name does not change.
- Existing tests (`test/*.el`) and docs (`docs/**`) that reference the old
  names are updated in the same pass; they break otherwise.

## 2. Structured errors: `LispSignal` / `LispThrow`

Today a failed Lisp `apply` collapses to `LispReplyValue::Error(String)`
(`format!("{e}")` in `lisp_requests::reply`) and is thrown to Clojure as
`emacs_jni.LispError`. The `emacs` crate already carries the real
structure: `emacs::Error` is `anyhow::Error` wrapping
`ErrorKind::Signal { symbol, data }` or `ErrorKind::Throw { tag, value }`,
each a `TempValue` bindable to a `Value` with the originating `Env`.

### Rust

- Replace `LispReplyValue::Error(String)` with two variants:
  - `Signal { symbol: Box<LispReplyValue>, data: Box<LispReplyValue> }`
  - `Throw { tag: Box<LispReplyValue>, value: Box<LispReplyValue> }`

  Each field is a nested, already-marshaled reply value (a symbol becomes
  the symbol marker, `data`/`value` typically a `LispObject`, a scalar its
  boxed value), produced by the existing `value_to_reply`.
- `lisp_requests::reply` takes `&Env`. On `Err(e)`:
  - `e.downcast_ref::<ErrorKind>()`:
    - `Signal { symbol, data }` → bind each `TempValue` to a `Value` with
      this `Env` (`unsafe TempValue::value`), marshal via `value_to_reply`,
      build `LispReplyValue::Signal`.
    - `Throw { tag, value }` → likewise → `LispReplyValue::Throw`.
    - other / non-`ErrorKind` → a `Signal` whose symbol is `error` and
      whose data is a one-element list holding the `Display` string, so
      every failure still arrives as a `LispSignal`.

  `TempValue::value` is sound here: `reply` runs on the Emacs main thread
  inside `service`, with the same `Env` the failing call used, before that
  `Env` is dropped.

### Java carrier (neutral, `emacs_jni.core`)

- Delete `LispError.java`. Add `LispNonLocalExit extends RuntimeException`
  carrying a `kind` discriminator (signal vs throw) and two `Object`
  fields (`symbol`/`data` or `tag`/`value`), each an already-marshaled
  value (marker, `LispObject`, or boxed scalar).
- Rust builds it and calls `completeExceptionally`. The carrier is neutral
  so Rust never touches the Clojure child classloader — the same reason
  the shim classes live in the platform loader.
- `src/shim.rs` (class table, `define_if_absent`, `find_class`) and
  `src/jvm_thread.rs` (`handle_lisp_reply`, `handle_invoke_reply`) build
  this carrier instead of `LispError`. `build.rs` compiles it.

### Clojure (`emacs-jni.clj`, the rich types)

`apply` / `apply-async` catch the `LispNonLocalExit` carrier (in the
`deref` path and the future's exceptional path) and rethrow as `ex-info`:

- signal → `(ex-info msg {:type ::lisp-signal :symbol sym :data data})`
- throw  → `(ex-info msg {:type ::lisp-throw :tag tag :value value})`

`symbol` / `data` / `tag` / `value` are `<-lisp`-converted (marker →
Clojure symbol/keyword; everything else passes through). Provide
predicates (`lisp-signal?`, `lisp-throw?`) and accessors. `ex-info` is the
representation because Clojure `deftype`/`defrecord` cannot extend
`Throwable` without an AOT `gen-class` step, which this project does not
have (Clojure loads from source at `require` time). `ex-info` gives real
catchable exceptions with the required fields in `ex-data`, no AOT.

## 3. Import a Lisp symbol or function as a Clojure callable

All new code in `emacs-jni.clj`.

### Late binding (symbol)

- `(defn require-symbol [sym])` → interns `sym` (a Clojure symbol, keyword,
  or string) to a `LispSymbol` marker and returns a `LispSymbolFn`.
- `LispSymbolFn` is a `deftype` in `emacs-jni.clj` implementing
  `clojure.lang.IFn`. It wraps the marker. Each `invoke` applies the
  marker through the existing apply path, so the function cell is
  re-resolved on every call (late binding — the `use_symbols!` analog).
  Wrapping (not returning a bare Clojure symbol) is required because a
  Clojure symbol's own `IFn` is map/set lookup.
- `(defmacro refer-symbols [& syms])` → for each `s`, emit
  `(def s (require-symbol 's))` in the current namespace.

Result: `((require-symbol 'message) "foo")`, and after
`(refer-symbols message)`, `(message "foo")`.

### Early binding (function)

- `(defn require-function [sym])` → resolves `(indirect-function sym)` now
  and wraps the returned function `LispObject` in a `LispFunctionFn`
  (`deftype`, `clojure.lang.IFn`). `indirect-function` chases symbol
  indirection, so the captured value is the real function object, not an
  alias symbol. Each `invoke` applies that captured function object —
  early binding (the `use_functions!` analog); a later redefinition of the
  Lisp symbol does not affect an already-imported function.
- `(defmacro refer-functions [& syms])` → analogous to `refer-symbols`.

Both wrapper types delegate `invoke` to the same `apply` used elsewhere;
they differ only in what they hold (a name marker vs a resolved function
object), which is what makes one late and the other early.

## 4. Testing

- Rust: extend `src/lisp_requests.rs` unit tests to cover marshaling the
  new `Signal` / `Throw` reply variants.
- Elisp (ert, `test/run-ert-tests.sh`): update namespace references in
  `test/jvm-clj-test.el` / `test/jvm-clojure-test.el`; add coverage for:
  - `require-symbol` / `refer-symbols` calling a real Lisp function
    (e.g. `+`, `format`).
  - `require-function` / `refer-functions` likewise, plus the early-vs-late
    difference (redefine the symbol, confirm the imported function does not
    change while the imported symbol does).
  - `LispSignal`: `(error "x")` → `::lisp-signal`, symbol `error`.
  - `LispThrow`: a function doing `(throw 'tag 5)` → `::lisp-throw`, tag
    `tag`, value `5`.
- Docs: update `docs/how-to-guides/access-lisp-from-clojure.md`,
  `docs/reference/lisp-api.md`, `docs/explanation/interop-design.md`, and
  add how-to material for the import macros and error handling.

## Risks

- `TempValue::value` safety depends on running in `reply` on the main
  thread with the originating `Env`; keep the marshaling inside `service`,
  not deferred.
- The Java package rename is broad; a missed `emacs_jni/X` path fails at
  class-definition or `find_class` time, which the existing start/smoke
  tests catch.
