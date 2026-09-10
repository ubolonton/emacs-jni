(ns emacs-jni.clj
  "Hold and manipulate Emacs Lisp values from Clojure.

  Scalars and strings (numbers, strings, nil, t) come back as Clojure
  values. Lisp symbols and keywords map to Clojure symbols and keywords,
  both directions. Every other Lisp value stays an opaque LispObject:
  inspect it via prin1-to-string, type-of, eq, and is-not-nil?.

  See design spec
  docs/superpowers/specs/2026-09-08-clojure-lisp-access-design.md."
  (:import [emacs_jni.core Lisp LispObject LispSymbol LispNonLocalExit]
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
    (reify BiFunction
      (apply [_ v t] (if t (throw-lisp-exit t) (<-lisp v))))))

(defn apply
  "Apply F to ARGS, blocking for the result. A Lisp non-local exit surfaces
  as an ex-info (::lisp-signal or ::lisp-throw)."
  [f & args]
  (try
    (<-lisp (deref (apply* f args)))
    (catch Throwable t (throw-lisp-exit t))))

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

(defn lisp-signal?
  "True if E is a Lisp-signal ex-info (from `apply`)."
  [e]
  (= (:type (ex-data e)) ::lisp-signal))

(defn lisp-throw?
  "True if E is a Lisp-throw ex-info (from `apply`)."
  [e]
  (= (:type (ex-data e)) ::lisp-throw))
