(ns dev.ublt.emacs.clj
  "Hold and manipulate Emacs Lisp values from Clojure, as opaque objects.
  Thin wrapper over emacs_jni.Lisp/LispObject -- see design spec
  docs/superpowers/specs/2026-09-08-clojure-lisp-access-design.md.

  Scalar and string results (numbers, strings, nil, t) come back as
  Clojure values. Every other Lisp value stays an opaque LispObject:
  inspect it via prin1-to-string, type-of, eq, and is-not-nil?, not by
  pulling Lisp data into Clojure data."
  (:import [emacs_jni Lisp LispObject]))

(defn intern
  "Intern S (a string) as a Lisp symbol, returning it as a LispObject."
  [s]
  (Lisp/intern s))

(defn apply-async
  "Apply F (a LispObject naming/holding a Lisp function) to ARGS
  asynchronously. Returns a CompletableFuture of the result: a Clojure
  value for a scalar or string result, else a LispObject."
  [f & args]
  (Lisp/applyAsync f (object-array args)))

(defn apply
  "Apply F to ARGS, blocking for the result. Sugar over
  @(apply-async f args...). A scalar or string result is a Clojure
  value; anything else is a LispObject."
  [f & args]
  (deref (Lisp/applyAsync f (object-array args))))

(defn type-of
  "Return V's Lisp type, as a LispObject (a type symbol)."
  [v]
  (Lisp/typeOf v))

(defn is-not-nil?
  "True unless V is the Lisp value nil."
  [v]
  (Lisp/isNotNil v))

(defn eq
  "True if A and B are the same Lisp object (Lisp's own eq)."
  [a b]
  (Lisp/eq a b))
