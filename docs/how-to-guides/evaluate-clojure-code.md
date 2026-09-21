# How to evaluate Clojure code

This guide assumes the JVM is `ready` and the Clojure jars are already
loaded. See
[Evaluate Clojure and connect an nREPL client](../tutorials/02-evaluate-clojure-and-connect-nrepl.md)
for the setup steps.

## Evaluate an expression

```elisp
(jvm-clj-eval-string "(+ 40 (* 2 1))")
;; => 42
```

`jvm-clj-eval-string` returns the result value directly and signals
`jvm-error` on failure.

## Require a namespace

```elisp
(jvm-clj-require "clojure.set")
;; => nil
```

`require` returns nil on success. Do this before evaluating code that
depends on that namespace.

## Read a Clojure error

A failing eval signals `jvm-error`, carrying Clojure's own exception
message:

```elisp
(condition-case err
    (jvm-clj-eval-string "(/ 1 0)")
  (jvm-error
   (message "%s" (cadr err))))   ; "... Divide by zero ..."
```

## Switch namespaces across calls

The bare `jvm-clj-eval-string` runs on the Emacs main thread, which
stays attached to one JVM thread between calls, so `*ns*` set by
`(in-ns ...)` in one call persists to the next.

The `-await` variant does **not** persist it: each
`jvm-clj-eval-string-await` call may run on a different underlying JVM
thread, and Clojure's current-namespace pointer is thread-local. If you
use `-await`, put `(in-ns ...)` and the code that depends on it inside
the same call, wrapped in a `do`:

```elisp
(jvm-clj-eval-string-await "(do (in-ns 'clojure.set) (ns-name *ns*))")
```

`(def x 5)` carries across `-await` calls either way — `def` mutates a
genuinely global `Var` — only an earlier `in-ns` (or any thread-local)
does not. See
[Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md)
for why.

## Get a printable form of a non-scalar result

```elisp
(jvm-call (jvm-clj-eval-string "[1 2 3]") "toString" nil)
;; => "[1 2 3]"
```

A scalar (number, string, boolean, nil) comes back as its Lisp value,
and a keyword or symbol result comes back as a Lisp keyword or symbol.
A vector, map, or lazy sequence comes back as an opaque wrapped JVM
object, not a Lisp-visible structure. Call `toString` on it through
`jvm-call`, as shown above, or evaluate `pr-str` around the original
expression instead:

```elisp
(jvm-clj-eval-string "(pr-str [1 2 3])")
;; => "[1 2 3]"
```

The second form returns a real Lisp string directly, since `pr-str`
itself returns a Java `String`, and strings marshal automatically.

## Go the other way

To have Clojure hold and manipulate Lisp values — call Lisp functions,
intern symbols, compare values — see
[How to access Emacs Lisp from Clojure](access-lisp-from-clojure.md).
