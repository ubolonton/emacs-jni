# How to evaluate Clojure code

This guide assumes the JVM is `ready` and the Clojure jars are already
loaded. See
[Evaluate Clojure and connect an nREPL client](../tutorials/02-evaluate-clojure-and-connect-nrepl.md)
for the setup steps.

## Evaluate an expression

```elisp
(jvm-clj-eval-string "(+ 40 (* 2 1))")
;; => (t 42 nil)
```

The result is `(OK RESULT ERR)`, the same shape `jvm-call-sync`
returns.

## Require a namespace

```elisp
(jvm-clj-require "clojure.set")
;; => (t nil nil)
```

Do this before evaluating code that depends on that namespace.

## Read a Clojure error

```elisp
(jvm-clj-eval-string "(/ 1 0)")
;; => (nil nil "... Divide by zero ...")
```

`OK` is `nil`. `ERR` holds Clojure's own exception message.

## Switch namespaces inside a single call

```elisp
(jvm-clj-eval-string "(do (in-ns 'clojure.set) (ns-name *ns*))")
```

Put `(in-ns ...)` and the code that depends on it inside the same
`jvm-clj-eval-string` call, wrapped in a `do` if you need more than
one form.

Do not rely on a namespace switch from one call carrying over to the
next call. Each `jvm-clj-eval-string` call may run on a different
underlying JVM thread, and Clojure's current-namespace pointer is
thread-local. `(def x 5)` in one call, then reading `x` in a later
call, does work — `def` mutates a genuinely global `Var`. Relying on an
earlier `in-ns` does not. See
[Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md)
for why.

If you do need a namespace switch (or any thread-local) to carry across
calls, use `jvm-clj-eval-string-direct` instead: it runs on the Emacs main
thread, which stays attached to one JVM thread between calls, so `*ns*` and
other thread-locals persist. It returns the value directly (signalling
`jvm-error` on failure) and blocks Emacs's main loop for the eval.

## Get a printable form of a non-scalar result

```elisp
(let ((result (jvm-clj-eval-string "[1 2 3]")))
  (jvm-call-sync (nth 1 result) "toString" nil))
;; => (t "[1 2 3]" nil)
```

A keyword, symbol, vector, map, or lazy sequence comes back as an
opaque wrapped JVM object, not a Lisp-visible structure. Call
`toString` on it through `jvm-call-sync`, as shown above, or evaluate
`pr-str` around the original expression instead:

```elisp
(jvm-clj-eval-string "(pr-str [1 2 3])")
;; => (t "[1 2 3]" nil)
```

The second form returns a real Lisp string directly, since `pr-str`
itself returns a Java `String`, and strings marshal automatically.

## Go the other way

To have Clojure hold and manipulate Lisp values — call Lisp functions,
intern symbols, compare values — see
[How to access Emacs Lisp from Clojure](access-lisp-from-clojure.md).
