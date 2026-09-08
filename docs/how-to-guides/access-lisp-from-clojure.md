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

## Symbols and keywords

Lisp symbols and keywords map to Clojure symbols and keywords, both
directions — so you pass and receive them as ordinary Clojure values.

```clojure
(l/intern "car")    ; => car   (a Clojure symbol)
(l/intern ":foo")   ; => :foo  (a Clojure keyword)
```

`intern` builds one from a string (a leading `:` makes it a keyword),
but a symbol/keyword literal works anywhere a Lisp symbol is wanted, so
`intern` is only needed for a name you compute at runtime.

Only *interned* Lisp symbols convert. An uninterned one
(`make-symbol`/`gensym`) stays an opaque `LispObject`, which preserves
its identity across the round trip. Clojure symbols have no interned
form of their own; each maps to the Lisp symbol interned under its
name.

## Apply a Lisp function

```clojure
(l/apply '+ 1 2)              ; => 3      (a Clojure Long)
(l/apply 'symbol-name 'car)   ; => "car"
```

`apply` blocks until Lisp returns. The result converts by type: a
scalar or string → the matching Clojure value; a Lisp symbol or keyword
→ a Clojure symbol/keyword; anything else (a list, map, buffer, ...) →
an opaque `LispObject`. Arguments convert the same way — a Clojure
symbol/keyword becomes a Lisp symbol/keyword, a scalar its Lisp
counterpart, and a `LispObject` passes by reference.

## Apply without blocking

```clojure
(l/apply-async '+ 1 2)   ; => CompletableFuture
@(l/apply-async '+ 1 2)  ; => 3
```

`apply-async` returns immediately with a `CompletableFuture`. Deref it
(`@`, or `.get`) for the result. `apply` is exactly `@(apply-async ...)`.

## Inspect a value

```clojure
(l/type-of "hello")   ; => string   (a Clojure symbol)
(l/type-of 42)        ; => integer
(l/is-not-nil? 'car)  ; => true
(l/is-not-nil? nil)   ; => false
(l/eq 'car 'car)      ; => true
```

`type-of` returns the Lisp type as a Clojure symbol. `is-not-nil?` and
`eq` return plain Clojure booleans.

## Read an opaque value's printed form

A non-scalar result stays a `LispObject`:

```clojure
(l/apply 'list 1 2 3)                   ; => a LispObject (a Lisp list)
(.prin1ToString (l/apply 'list 1 2 3))  ; => "(1 2 3)"
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
