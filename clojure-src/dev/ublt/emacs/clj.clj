(ns dev.ublt.emacs.clj
  "Hold and manipulate Emacs Lisp values from Clojure.

  Scalars and strings (numbers, strings, nil, t) come back as Clojure
  values. Lisp symbols and keywords map to Clojure symbols and keywords,
  both directions. Every other Lisp value stays an opaque LispObject:
  inspect it via prin1-to-string, type-of, eq, and is-not-nil?.

  See design spec
  docs/superpowers/specs/2026-09-08-clojure-lisp-access-design.md."
  (:import [emacs_jni Lisp LispObject LispSymbol]
           [java.util.function Function]))

(defn- ->lisp
  "Marshal a value heading into Lisp: a Clojure symbol or keyword becomes
  a LispSymbol marker the native side interns; everything else (scalars,
  LispObjects, other objects) passes through unchanged."
  [x]
  (cond
    (keyword? x) (LispSymbol. (subs (str x) 1) true)
    (symbol? x)  (LispSymbol. (str x) false)
    :else x))

(defn- <-lisp
  "Convert a native result: a LispSymbol marker becomes a Clojure
  symbol/keyword; everything else passes through."
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
  (<-lisp (Lisp/intern s)))

(defn apply-async
  "Apply F to ARGS asynchronously. Returns a CompletableFuture of the
  result (converted): a scalar/string/symbol/keyword as a Clojure value,
  anything else a LispObject."
  [f & args]
  (.thenApply (apply* f args) (reify Function (apply [_ v] (<-lisp v)))))

(defn apply
  "Apply F to ARGS, blocking for the result. Sugar over
  @(apply-async f args...)."
  [f & args]
  (<-lisp (deref (apply* f args))))

(defn type-of
  "Return V's Lisp type as a Clojure symbol (e.g. symbol, string, cons)."
  [v]
  (<-lisp (Lisp/typeOf (->lisp v))))

(defn is-not-nil?
  "True unless V is the Lisp value nil."
  [v]
  (Lisp/isNotNil (->lisp v)))

(defn eq
  "True if A and B are the same Lisp object (Lisp's own eq)."
  [a b]
  (Lisp/eq (->lisp a) (->lisp b)))
