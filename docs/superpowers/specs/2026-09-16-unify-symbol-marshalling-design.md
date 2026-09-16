# Unify symbol/keyword marshalling — design

Experimental. Additive to the generic JVM bridge (`jvm-call*`) and to
`jvm-clj-eval-string`. See the plan
`docs/superpowers/plans/2026-09-16-unify-symbol-marshalling-plan.md` for phase
status.

## Problem

Two marshalling systems exist, in opposite directions, and only one knows
symbols:

- **Generic bridge** — `LispArg` (`src/commands.rs:17`), used by `jvm-call*`
  and therefore `jvm-clj-eval-string`. No symbol/keyword concept. IN
  (`lisp_value_to_arg`, `src/lib.rs:374-380`) *signals* on a keyword or
  non-`t` symbol. OUT (`object_to_lisp_arg`, `src/marshal.rs:328`) wraps any
  unrecognised object — a `clojure.lang.Keyword` included — as an opaque
  handle.
- **Clojure Lisp-access** — `LispReplyValue::Symbol` (`src/commands.rs:59`)
  plus the `emacs_jni.core.LispSymbol` marker class. The only place
  keyword/symbol ↔ Lisp conversion lives, split between native code
  (`src/lisp_requests.rs:110`, `src/jvm_thread.rs:631`) and the Clojure
  `->lisp`/`<-lisp` (`clojure-src/emacs_jni/clj.clj:14-30`).

So `((require-function 'identity) :a)` returns `:a` in Clojure (Lisp-access
path, `<-lisp`), but `(jvm-clj-eval-string ":a")` returns an opaque user
pointer in Lisp (generic path, no keyword arm).

## Goal

Give the generic bridge the same symbol/keyword capability, both directions,
through the existing `LispSymbol` marker class. The core learns only its **own**
marker class; `clojure.lang.*` knowledge stays in the Clojure layer, which
converts to/from the marker (as `->lisp`/`<-lisp` already do). After this:

- `(jvm-clj-eval-string ":a")` → `:a`; `"'foo"` → `foo`.
- A Lisp keyword/symbol may be passed as any `jvm-call*` argument (today an
  error).

Conversion stays shallow, matching today: a collection result (`[:a :b]`, a
map) still returns as an opaque handle.

## Design

### New `LispArg::Symbol { name, keyword }`

Add to `src/commands.rs`, mirroring `LispReplyValue::Symbol`. `name` is the
bare name (no leading `:`). Every `LispArg` match site must handle it
(`lisp_arg_to_jvalue`, `lisp_arg_to_sexp`).

### IN: Lisp → JVM (`lisp_value_to_arg`, `src/lib.rs`)

Rule: **a symbol is data — always a marker. Only a function *object* is a
callback.** This matches the Clojure side, where `->lisp` maps every symbol to
a `LispSymbol` marker and callability is opt-in via `require-symbol`. A quoted
symbol must not change Java type based on whether it happens to hold a function
cell.

Add two arms and reorder so `symbolp` precedes `functionp`:

- `keywordp` → `Symbol { keyword: true }` (before the generic `symbolp` arm,
  else a keyword would take `keyword: false`).
- `t` → `True` (already present; keep before the generic `symbolp` arm, since
  `t` is a symbol).
- `symbolp` → `Symbol { keyword: false }`, placed **before** `functionp`.

Effect: `'car`/`#'car` (a bare function symbol; `#'car` evaluates to the symbol
`car`) now becomes a marker, not a callback. The `functionp` arm then catches
only non-symbol function objects — lambdas, closures, byte-code, subrs — which
stay callbacks, unchanged. The final `jvm_invalid_argument` signal still covers
vectors, hash tables, etc.

Full chain: `nil` → `keywordp` → `t` → int → bignum → float → string → handle
→ `symbolp` → `functionp` → error.

`lisp_arg_to_jvalue` (`src/marshal.rs`) gains a `Symbol` arm that builds an
`emacs_jni/core/LispSymbol` JObject, copying `src/jvm_thread.rs:631-639`
(`new_string` → `find_class` → `new_object` with
`(java.lang.String, boolean)`).

### OUT: JVM → Lisp

Recognition happens once, in `object_to_lisp_arg` (`src/marshal.rs`): before the
opaque-handle fallback, if the object
`is_instance_of emacs_jni/core/LispSymbol`, read its public `name`
(`Ljava/lang/String;`) and `keyword` (`Z`) fields → `LispArg::Symbol`.

That `LispArg` is then turned into a Lisp value at **two** sites — the crate has
two independent OUT converters, and both must handle the new variant:

1. **Direct path** — `lisp_requests::lisp_arg_to_value` (`src/lisp_requests.rs:131`),
   used by `jvm--call-direct` (`src/lib.rs:188`). This is the path
   `jvm-clj-eval-string` (the `-direct` alias) uses, so it is what actually
   fixes the reported case. Add a `Symbol` arm that interns on the main thread,
   mirroring the existing `Intern` request (`src/lisp_requests.rs:110-117`):

   ```rust
   LispArg::Symbol { name, keyword } => {
       if *keyword { env.intern(&format!(":{name}")) } else { env.intern(name) }
   }
   ```

2. **Async wire path** — `lisp_arg_to_sexp` (`src/marshal.rs`) renders a new tag
   `(jvm-symbol "name" t|nil)` (name quoted via `quote_lisp_string`), and
   `jvm--resolve-sexp-result` (`elisp/jvm.el:121`) gains a `pcase` arm:

   ```elisp
   (`(jvm-symbol ,name ,keyword)
    (intern (if keyword (concat ":" name) name)))
   ```

   This resolver runs on both `call-done` results and `invoke` (JVM→Lisp
   callback) arguments, so the wire path is symmetric for free. Used by
   `jvm-clj-eval-string-await`.

### eval-string routes through the Clojure layer

`clojure.core/eval` of `:a` returns a `clojure.lang.Keyword`, which the core
does not (by design) recognise. So `jvm-clj-eval-string-{direct,await}` resolve
a new public var `emacs-jni.clj/eval-for-lisp` instead of `clojure.core/eval`:

```clojure
(defn eval-for-lisp
  "Evaluate FORM and marshal the result toward Lisp: a keyword/symbol result
  becomes a LispSymbol marker the generic bridge interns; every other value
  passes through the ordinary jvm-call return marshalling. Backs
  `jvm-clj-eval-string'."
  [form]
  (->lisp (eval form)))
```

`->lisp` (`clj.clj:14`) turns a keyword/symbol into a marker; the OUT arm above
interns it. Scalars/strings/collections pass through unchanged.

The `jvm-clj--invoke-with-form-{direct,await}` helpers change their
`var-ns`/`var-name` from `"clojure.core"`/`"eval"` to
`"emacs-jni.clj"`/`"eval-for-lisp"`; the require path (`clojure.core`/`require`)
is untouched.

## Consequence: eval-string now needs emacs-jni.clj loaded

`jvm-clj-start` already loads `emacs-jni.clj` (and puts the source dir on the
classpath). But the two eval test files bootstrap by hand and skip start, so
they would break once eval-string resolves `emacs-jni.clj/eval-for-lisp`:

- `test/jvm-clj-eval-test.el` `ensure-ready` (line 23) — used by the `-await`
  eval and nREPL tests.
- `test/jvm-clj-eval-direct-test.el` `ensure-ready` (line 17) — used by the
  `-direct` eval tests.

Fix (both files, surgical): after the existing jar loop, add
`(jvm-load-jar-await jvm-clj--source-dir)` and
`(jvm-clj-require-await "emacs-jni.clj")`. `addURL` is lazy/append and a loader
caches its defined classes, so the nREPL test's later re-entry into
`jvm-clj-start-await` (which re-adds the source dir) is a harmless no-op.

## Components

- `src/commands.rs` — `LispArg::Symbol` variant.
- `src/lib.rs` — keyword/symbol arms in `lisp_value_to_arg`.
- `src/marshal.rs` — `Symbol` arm in `lisp_arg_to_jvalue`; `LispSymbol`
  recognition in `object_to_lisp_arg`; `jvm-symbol` render in
  `lisp_arg_to_sexp`.
- `src/lisp_requests.rs` — `Symbol` arm in `lisp_arg_to_value` (direct-path OUT
  converter).
- `elisp/jvm.el` — `jvm-symbol` arm in `jvm--resolve-sexp-result` (async-path
  OUT converter).
- `clojure-src/emacs_jni/clj.clj` — public `eval-for-lisp`.
- `elisp/jvm-clj.el` — `jvm-clj-eval-string-{direct,await}` resolve
  `emacs-jni.clj/eval-for-lisp`.

## Testing

- Rust unit (`src/marshal.rs`): `lisp_arg_to_sexp(Symbol {keyword:true})` →
  `(jvm-symbol "foo" t)`; `false` → `(jvm-symbol "foo" nil)`.
- Elisp integration, generic bidirectional path via `clojure.core/identity`
  (echoes the marker, no Clojure conversion involved):
  `(jvm-call-direct (jvm-clj--var-direct "clojure.core" "identity") "invoke"
  (list :a))` → `:a`; with `'foo` → `foo`; with a function symbol `'car` →
  `car` (locks the "symbol is data, not a callback" rule). Exercises IN
  (keyword/symbol → marker) and OUT (marker → keyword/symbol) together.
- Elisp integration, Clojure-layer path: `(jvm-clj-eval-string ":a")` → `:a`;
  `"'foo"` → `foo`. Async parity: same via `jvm-clj-eval-string-await`.
- Existing scalar (`(+ 40 (* 2 1))` → `42`) and error (`(/ 1 0)`) eval tests
  still pass after the `ensure-ready` change, in both eval test files.

## Risks and tradeoffs

- `object_to_lisp_arg` adds one `find_class` + `is_instance_of` per object
  return. Negligible for the prototype; cache later if it matters.
- `eval-for-lisp` ties `jvm-clj-eval-string` to `emacs-jni.clj`. Deliberate:
  Lisp-native keyword/symbol results are an `emacs-jni.clj` feature, and the
  real entry (`jvm-clj-start`) already loads it.
- Shallow conversion only: collections of keywords stay opaque handles,
  matching the documented limit on both paths.
- Behaviour change: a bare function symbol passed as a `jvm-call*` argument
  (`'car`/`#'car`) now marshals to a marker, not a callback proxy — Java gets a
  non-callable marker if it expected to call it back (a Java-side failure, not
  silent corruption). No in-repo caller does this; the callback test uses a
  `lambda`. Callability of a named function stays available via the Clojure
  `require-symbol`/`require-function` wrappers.
