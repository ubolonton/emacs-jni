(ns emacs-jni.clj
  "Hold and manipulate Emacs Lisp values from Clojure.

  Scalars and strings (numbers, strings, nil, t) come back as Clojure
  values. Lisp symbols and keywords map to Clojure symbols and keywords,
  both directions. Every other Lisp value stays an opaque LispObject:
  inspect it via prin1-to-string, type-of, eq, and is-not-nil?.

  See design spec
  docs/superpowers/specs/2026-09-08-clojure-lisp-access-design.md."
  (:import [emacs_jni.core Lisp LispObject LispSymbol LispNonLocalExit LispSignal LispThrow]
           [java.util.function BiFunction]))

(defn- ->lisp
  "Marshal a value heading into Lisp: a Clojure symbol or keyword becomes
  a LispSymbol marker the native side interns; everything else (scalars,
  LispObjects, other objects) passes through unchanged."
  [x]
  (cond
    (keyword? x) (LispSymbol. (subs (str x) 1) true)
    (symbol? x)  (LispSymbol. (str x) false)
    :else x))

(defn ->clj
  "Convert a marshalled Lisp value (a LispSymbol marker, LispObject, or
  boxed scalar) to its Clojure form: a marker becomes a symbol/keyword,
  everything else passes through."
  [x]
  (if (instance? LispSymbol x)
    (if (.-keyword x) (keyword (.-name x)) (symbol (.-name x)))
    x))

(defn- apply*
  "Raw native applyAsync with args marshalled into Lisp. Returns a
  CompletableFuture of the still-unconverted result."
  [f args]
  (Lisp/applyAsync (->lisp f) (object-array (map ->lisp args))))

(defn intern
  "Intern S (a string) as a Lisp symbol/keyword and return it as a Clojure
  symbol/keyword. A leading \":\" makes it a keyword."
  [s]
  (->clj (Lisp/intern s)))

(defn eval-for-lisp
  "Read CODE (a string of Clojure source) with the full Clojure reader, eval
  it, and marshal the result toward Lisp: a keyword or symbol result becomes a
  LispSymbol marker the generic bridge interns back into a Lisp keyword/symbol;
  every other value passes through the ordinary jvm-call return marshalling.
  Backs `jvm-clj-eval-string'.

  Uses `load-string`, which reads with the full Clojure reader (reader macros
  `'`, backtick, `~`, `@`, `#()`, `#\"re\"` all work) and evaluates every
  top-level form, returning the last one's value. This is why CODE is passed as
  a raw string rather than pre-read: `clojure.java.api.Clojure/read` reads
  EDN-only and would reject the reader macros."
  [code]
  (->lisp (load-string code)))

(defn- throw-lisp-exit
  "Given a completion throwable (a bare LispNonLocalExit from .handle, or one
  wrapped in an ExecutionException from deref), rethrow the underlying
  LispSignal/LispThrow so a caller can `catch' it by type. Anything else is
  rethrown unchanged."
  [t]
  (let [cause (if (instance? java.util.concurrent.ExecutionException t) (.getCause t) t)]
    (throw (if (instance? LispNonLocalExit cause) cause t))))

(defn apply-async
  "Apply F to ARGS asynchronously. Returns a CompletableFuture of the
  converted result. A Lisp non-local exit surfaces as a LispSignal/LispThrow."
  [f & args]
  (.handle (apply* f args)
    (reify BiFunction
      (apply [_ v t] (if t (throw-lisp-exit t) (->clj v))))))

(defn apply
  "Apply F to ARGS, blocking for the result. A Lisp non-local exit surfaces
  as a LispSignal/LispThrow."
  [f & args]
  (try
    (->clj (deref (apply* f args)))
    (catch Throwable t (throw-lisp-exit t))))

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
  a LispSignal/LispThrow, same as `apply`."
  [receiver args]
  (try
    (->clj (deref (apply* receiver args)))
    (catch Throwable t (throw-lisp-exit t))))

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

(defn type-of
  "Return V's Lisp type as a Clojure symbol (e.g. symbol, string, cons)."
  [v]
  (->clj (Lisp/typeOf (->lisp v))))

(defn is-not-nil?
  "True unless V is the Lisp value nil."
  [v]
  (Lisp/isNotNil (->lisp v)))

(defn eq
  "True if A and B are the same Lisp object (Lisp's own eq)."
  [a b]
  (Lisp/eq (->lisp a) (->lisp b)))
