# How to evaluate Clojure code

This guide assumes that you called `jvm-clj-start` with the Clojure jars on `jvm-clj-classpath`.
See [Start the Clojure side](access-lisp-from-clojure.md#start-the-clojure-side).

## Evaluate an expression

```elisp
(jvm-clj-eval-string "(+ 40 (* 2 1))")
;; => 42
```

`jvm-clj-eval-string` evaluates all top-level forms and returns the value of the last form. It
signals `jvm-error` on failure.

## Require a namespace

```elisp
(jvm-clj-require "clojure.set")
;; => nil
```

Require a namespace before you evaluate code that uses it.

## Read a Clojure error

A failed evaluation signals `jvm-error`. The message contains the Clojure exception message:

```elisp
(condition-case err
    (jvm-clj-eval-string "(/ 1 0)")
  (jvm-error
   (message "%s" (cadr err))))   ; contains "Divide by zero"
```

## Switch namespaces across calls

An `in-ns` in one `jvm-clj-eval-string` call does not stay in effect for the next call. Put
`in-ns` and the code that uses it in one call:

```elisp
(jvm-clj-eval-string "(in-ns 'clojure.set) (ns-name *ns*)")
;; => clojure.set
```

A `def` stays in effect across calls. See
[Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md).

## Read a collection result

A number, string, boolean, or `nil` returns as a Lisp value. A keyword or symbol returns as a Lisp
keyword or symbol. A vector, map, or sequence returns as a wrapped JVM object.

To get a string, wrap the expression in `pr-str`:

```elisp
(jvm-clj-eval-string "(pr-str [1 2 3])")
;; => "[1 2 3]"
```

Or call `toString` on the wrapped object:

```elisp
(jvm-call (jvm-clj-eval-string "[1 2 3]") "toString" nil)
;; => "[1 2 3]"
```

## Use Lisp values from Clojure

See [How to access Emacs Lisp from Clojure](access-lisp-from-clojure.md).
