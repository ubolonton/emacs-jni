(ns emacs-jni.clj
  "Hold and manipulate Emacs Lisp values from Clojure.

  Lisp numbers, strings, `nil`, and `t` come back as Clojure values (`t`
  becomes `true`). Lisp symbols and keywords map to Clojure symbols and
  keywords, in both directions. Every other Lisp value stays an opaque
  `LispObject`. Inspect it with `.toString` (Lisp `prin1-to-string`),
  [[type-of]], [[eq]], and [[is-not-nil?]].

  [[intern]], [[type-of]], [[is-not-nil?]], and [[eq]] block until Emacs
  replies. If Emacs does not reply in 30 seconds, they throw
  `java.util.concurrent.TimeoutException`.

  See `docs/how-to-guides/access-lisp-from-clojure.md`."
  (:refer-clojure :exclude [intern apply])
  (:import [emacs_jni.core Lisp LispSymbol LispNonLocalExit]
           [java.util.function BiFunction]))

(defn- ->lisp
  "Converts `x` for a call into Lisp.

  A Clojure symbol or keyword becomes a `LispSymbol`, which the native side
  interns. Other values (scalars, `LispObject`s, other objects) pass through
  unchanged."
  [x]
  (cond
    (keyword? x) (LispSymbol. (subs (str x) 1) true)
    (symbol? x)  (LispSymbol. (str x) false)
    :else x))

(defn ->clj
  "Converts `x`, a value from Lisp, to its Clojure form.

  A `LispSymbol` becomes a Clojure symbol or keyword. Other values (a
  `LispObject` or a boxed scalar) pass through unchanged."
  [x]
  (if (instance? LispSymbol x)
    (if (.-keyword x) (keyword (.-name x)) (symbol (.-name x)))
    x))

(defn- apply*
  "Calls `Lisp/applyAsync` with `f` and `args` converted by [[->lisp]].

  Returns a `CompletableFuture` of the result before [[->clj]] conversion."
  [f args]
  (Lisp/applyAsync (->lisp f) (object-array (map ->lisp args))))

(defn intern
  "Interns the string `s` in Lisp and returns it as a Clojure symbol.

  A leading `:` makes a keyword. Blocks until Emacs replies, and throws
  `java.util.concurrent.TimeoutException` after 30 seconds."
  [s]
  (->clj (Lisp/intern s)))

(defn eval-for-lisp
  "Evaluates the Clojure source string `code` and converts the result for Lisp.

  Backs the Elisp `jvm-clj-eval-string`. A keyword or symbol result becomes
  a `LispSymbol`.

  [[clojure.core/load-string]] uses the full reader, evaluates every
  top-level form, and returns the last value. `code` stays a string because
  `clojure.java.api.Clojure/read` reads only EDN and rejects reader macros."
  [code]
  (->lisp (load-string code)))

(defn- throw-lisp-exit
  "Rethrows the `LispSignal` or `LispThrow` in `t`, so a caller can `catch` it.

  `t` is a bare `LispNonLocalExit` (from `.handle`) or one wrapped in an
  `ExecutionException` (from `deref`). Any other throwable is rethrown
  unchanged."
  [t]
  (let [cause (if (instance? java.util.concurrent.ExecutionException t) (.getCause t) t)]
    (throw (if (instance? LispNonLocalExit cause) cause t))))

(defn apply-async
  "Applies the Lisp function `f` to `args` without blocking.

  Returns a `CompletableFuture` of the converted result. A Lisp `signal` or
  `throw` completes the future exceptionally with a `LispSignal` or
  `LispThrow`."
  [f & args]
  (.handle (apply* f args)
    (reify BiFunction
      (apply [_ v t] (if t (throw-lisp-exit t) (->clj v))))))

(defn apply
  "Applies the Lisp function `f` to `args` and returns the converted result.

  Blocks with no time limit. A Lisp `signal` or `throw` is thrown as a
  `LispSignal` or `LispThrow`. See [[apply-async]] for the non-blocking form."
  [f & args]
  (try
    (->clj (deref (apply* f args)))
    (catch Throwable t (throw-lisp-exit t))))

(defn- sym->lisp-name
  "Returns the Lisp symbol name for `sym`, a Clojure symbol, keyword, or string.

  A namespace segment stays in the name, so `foo/bar` becomes the Lisp name
  `foo/bar`."
  [sym]
  (if-let [ns (and (or (symbol? sym) (keyword? sym)) (namespace sym))]
    (str ns "/" (name sym))
    (name sym)))

(defn- run-lisp
  "Applies `receiver` to the seq `args` and returns the converted result.

  `receiver` is a `LispSymbol` or a function `LispObject`. Behaves as
  [[apply]]: blocks with no time limit, and throws a `LispSignal` or
  `LispThrow`."
  [receiver args]
  (try
    (->clj (deref (apply* receiver args)))
    (catch Throwable t (throw-lisp-exit t))))

(defmacro ^:private def-lisp-callable
  "Defines a `deftype` `name` that applies its `receiver` with [[run-lisp]].

  Implements arities 0-5 directly. `applyTo` covers [[clojure.core/apply]]
  and higher arities."
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

;; Late binding: wraps a `LispSymbol`. Each call `funcall`s the symbol, so a
;; later redefinition takes effect.
(def-lisp-callable LispSymbolFn)

;; Early binding: wraps the resolved function object. Each call applies that
;; same object.
(def-lisp-callable LispFunctionFn)

(defn require-symbol
  "Returns a `LispSymbolFn` that calls the Lisp function named `sym`.

  Late binding, as `emacs::use_symbols!` in Rust: each call `funcall`s the
  symbol, so a later redefinition takes effect."
  [sym]
  (->LispSymbolFn (LispSymbol. (sym->lisp-name sym) false)))

(defmacro refer-symbols
  "Defines one var per symbol in `syms`, bound to [[require-symbol]] of it.

  For example, `(refer-symbols message)` makes `(message \"hi\")` call the
  Lisp `message`."
  [& syms]
  `(do ~@(map (fn [s] `(def ~s (require-symbol '~s))) syms)))

(defn require-function
  "Returns a `LispFunctionFn` that calls the current Lisp function of `sym`.

  Early binding, as `emacs::use_functions!` in Rust: resolves
  `indirect-function` now and keeps the function object. A later
  redefinition of `sym` has no effect."
  [sym]
  (let [f (apply (LispSymbol. "indirect-function" false)
                 (LispSymbol. (sym->lisp-name sym) false))]
    (->LispFunctionFn f)))

(defmacro refer-functions
  "Defines one var per symbol in `syms`, bound to [[require-function]] of it."
  [& syms]
  `(do ~@(map (fn [s] `(def ~s (require-function '~s))) syms)))

(defn type-of
  "Returns the Lisp type of `v` as a Clojure symbol, e.g. `cons` or `string`.

  Blocks until Emacs replies, and throws
  `java.util.concurrent.TimeoutException` after 30 seconds."
  [v]
  (->clj (Lisp/typeOf (->lisp v))))

(defn is-not-nil?
  "Returns true unless `v` is the Lisp value `nil`.

  Blocks until Emacs replies, and throws
  `java.util.concurrent.TimeoutException` after 30 seconds."
  [v]
  (Lisp/isNotNil (->lisp v)))

(defn eq
  "Returns true if `a` and `b` are the same Lisp object, by Lisp `eq`.

  Blocks until Emacs replies, and throws
  `java.util.concurrent.TimeoutException` after 30 seconds."
  [a b]
  (Lisp/eq (->lisp a) (->lisp b)))
