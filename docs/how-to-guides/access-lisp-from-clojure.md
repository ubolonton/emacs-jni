# How to access Emacs Lisp from Clojure

This guide covers the other direction of interop: Clojure code that
holds and manipulates Emacs Lisp values directly. It uses the
`jvm-clj.el` library and the `dev.ublt.emacs.clj` Clojure namespace.
It assumes you can already evaluate Clojure — see
[How to evaluate Clojure code](evaluate-clojure-code.md).

The Clojure snippets below run wherever Clojure runs: at an nREPL
client connected to your Emacs, or inside a `jvm-clojure-eval-string`
call.

## Start the Clojure side

```elisp
(require 'jvm-clj)
(setq jvm-clj-classpath
      (directory-files (getenv "EMACS_JNI_CLOJURE_JARS_DIR") t "\\.jar\\'"))
(jvm-clj-start)
```

`jvm-clj-start` starts the JVM if needed, loads `jvm-clj-classpath`
plus this package's own `clojure-src` directory, and requires
`dev.ublt.emacs.clj`. It signals an error if any step fails — for
example, if no Clojure jar is on `jvm-clj-classpath`.

To start an nREPL server with the namespace already loaded, use
`jvm-clj-nrepl-start` in place of `jvm-nrepl-start`.

## Alias the namespace

`dev.ublt.emacs.clj` defines `intern`, `apply`, and `eq`, which shadow
`clojure.core`. Alias it rather than referring the names in:

```clojure
(require '[dev.ublt.emacs.clj :as l])
```

## Intern a symbol

```clojure
(l/intern "car")
;; => #object[emacs_jni.LispObject ...]
```

`intern` returns a `LispObject` — an opaque handle to a Lisp value. It
is not a Clojure value; inspect it with the functions below.

## Apply a Lisp function

```clojure
(l/apply (l/intern "+") 1 2)   ; => 3  (a Clojure Long)
```

`apply` blocks until Lisp returns. A scalar or string result — number,
string, `t`, `nil` — comes back as the matching Clojure value, so you
use it directly. Any other Lisp value (a symbol, list, map, ...) comes
back as an opaque `LispObject`. Arguments may be `LispObject`s or plain
Clojure scalars, which convert to the matching Lisp value.

## Apply without blocking

```clojure
(l/apply-async (l/intern "+") 1 2)   ; => CompletableFuture
@(l/apply-async (l/intern "+") 1 2)  ; => 3
```

`apply-async` returns immediately with a `CompletableFuture`. Deref it
(`@`, or `.get`) for the result. `apply` is exactly `@(apply-async ...)`.

## Inspect a value

```clojure
(l/type-of (l/intern "car"))               ; => a LispObject: symbol
(l/is-not-nil? (l/intern "car"))           ; => true
(l/is-not-nil? (l/intern "nil"))           ; => false
(l/eq (l/intern "car") (l/intern "car"))   ; => true
```

`type-of` returns a `LispObject` (a type symbol). `is-not-nil?` and
`eq` return plain Clojure booleans.

## Read an opaque value's printed form

A non-scalar result stays a `LispObject`:

```clojure
(l/apply (l/intern "list") 1 2 3)                   ; => a LispObject (a Lisp list)
(.prin1ToString (l/apply (l/intern "list") 1 2 3))  ; => "(1 2 3)"
```

`LispObject.prin1ToString` returns the value's `prin1-to-string`
output, as a Java `String` — the way to read a value that stays opaque
(see
[Interop design](../explanation/interop-design.md#why-a-lisp-value-becomes-an-opaque-java-object)).

## Handle a Lisp error

```clojure
(l/apply (l/intern "error") "boom")
;; throws: the future completes exceptionally with emacs_jni.LispError,
;; which surfaces on deref (wrapped in a java.util.concurrent.ExecutionException)
```

A Lisp `error` signaled during `apply`/`apply-async` completes the
future exceptionally with an `emacs_jni.LispError`. It surfaces when
you deref — mirroring how a Java-side callback error surfaces to Lisp
(see [How to pass a Lisp function to Java](pass-a-lisp-function-to-java.md)).

## Know when a LispObject stops holding its value

A `LispObject` keeps its underlying Lisp value alive for as long as the
`LispObject` itself is reachable on the JVM side. Once it is garbage
collected, its Lisp reference is freed. Do not build a reference cycle
that spans both runtimes — see
[Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md#lisp-values-held-by-clojure-are-opaque).

## Note on timing

Each of these calls blocks a JVM thread until Emacs services it on its
main thread. If Emacs is busy in a long Lisp computation, the call
waits until Emacs is idle again. See
[Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md#a-synchronous-clojure-to-lisp-call-waits-for-emacs-to-be-free).
