# How to access Emacs Lisp from Clojure

This guide uses the `emacs-jni.clj` Clojure namespace to call Lisp functions and hold Lisp values.
It assumes that you can evaluate Clojure. See
[How to evaluate Clojure code](evaluate-clojure-code.md).

The Clojure examples run in an nREPL client connected to Emacs, or in a `jvm-clj-eval-string`
call.

## Start the Clojure side

```elisp
(require 'jvm-clj)
(setq jvm-clj-classpath
      (directory-files (getenv "EMACS_JNI_CLOJURE_JARS_DIR") t "\\.jar\\'"))
(jvm-clj-start)
```

`jvm-clj-start` does these steps:

1. It starts the JVM if the status is not `ready`.
2. It loads each entry of `jvm-clj-classpath`, and the `clojure-src` directory of this package.
3. It requires `emacs-jni.clj`.

It signals an error if a step fails, for example if `jvm-clj-classpath` has no Clojure jar.

`jvm-clj-nrepl-start` calls `jvm-clj-start`, then starts an nREPL server.

## Alias the namespace

`emacs-jni.clj` defines `intern`, `apply`, and `eq`. These names shadow `clojure.core`. Use an
alias, not `:refer`:

```clojure
(require '[emacs-jni.clj :as l])
```

## Pass a Lisp symbol or keyword

Use a Clojure symbol or keyword. It converts to the Lisp symbol or keyword with the same name.
Lisp symbols and keywords return as Clojure symbols and keywords.

To make one from a string at run time, use `intern`. A leading `:` makes a keyword:

```clojure
(l/intern "car")    ; => car   (a Clojure symbol)
(l/intern ":foo")   ; => :foo  (a Clojure keyword)
```

An uninterned Lisp symbol, from `make-symbol` or `gensym`, returns as a `LispObject`.

## Apply a Lisp function

```clojure
(l/apply '+ 1 2)              ; => 3      (a Clojure Long)
(l/apply 'symbol-name 'car)   ; => "car"
```

`apply` blocks until Lisp returns. Results convert as follows:

| Lisp value | Clojure value |
|---|---|
| number, string, `nil`, `t` | the matching Clojure value |
| symbol, keyword | Clojure symbol, keyword |
| other values (list, buffer, ...) | `LispObject` |

Arguments convert in the opposite direction. A `LispObject` argument gives the Lisp value that it
holds.

Lisp code runs only on the Emacs main thread:

- In a direct call, such as `jvm-clj-eval-string`, `apply` runs the Lisp function immediately.
- On other threads, such as nREPL threads, Emacs runs the Lisp function when it is idle, or while
  it waits in an `-await` call.
- On other threads, `apply` and the `require-*`/`refer-*` functions wait with no timeout.
  `intern`, `type-of`, `eq`, `is-not-nil?`, and `prin1ToString` wait up to 30 seconds.

See [Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md).

## Apply a Lisp function without blocking

```clojure
(l/apply-async '+ 1 2)   ; => a CompletableFuture
@(l/apply-async '+ 1 2)  ; => 3
```

`apply-async` returns a `CompletableFuture` immediately. Use `deref` (`@`) or `.get` to get the
result.

## Import a Lisp function as a Clojure function

Use `require-symbol` or `refer-symbols` if Lisp code can redefine the function later, for example
with `defun` or `advice-add`. Each call uses the current definition:

```clojure
((l/require-symbol 'message) "hi")

(l/refer-symbols message)
(message "hi")
```

`refer-symbols` defines one var for each symbol, with the same name, in the current namespace.

Use `require-function` or `refer-functions` to keep one definition. They resolve
`indirect-function` once, when you import the name:

```clojure
(l/refer-functions message)
(message "hi")
```

`refer-symbols` and `refer-functions` accept only names that are valid Clojure symbols. For other
names, give a string to `require-symbol` or `require-function`:

```clojure
(def my-fn (l/require-symbol "some-odd-name"))
```

## Inspect a value

```clojure
(l/type-of "hello")   ; => string   (a Clojure symbol)
(l/type-of 42)        ; => integer
(l/is-not-nil? 'car)  ; => true
(l/is-not-nil? nil)   ; => false
(l/eq 'car 'car)      ; => true
```

`type-of` returns the Lisp type as a Clojure symbol. `is-not-nil?` and `eq` return Clojure
booleans.

## Read an opaque value's printed form

```clojure
(l/apply 'list 1 2 3)                   ; => a LispObject (a Lisp list)
(.prin1ToString (l/apply 'list 1 2 3))  ; => "(1 2 3)"
```

`LispObject.prin1ToString` returns the `prin1-to-string` output as a Java `String`.

A `LispObject` keeps its Lisp value alive while the JVM can reach the `LispObject`. Do not make
reference cycles across Lisp and the JVM. See
[Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md).

## Handle a Lisp error

A Lisp non-local exit in `apply` throws one of these exceptions:

| Lisp exit | Class | Fields |
|---|---|---|
| `signal`, `error` | `emacs_jni.core.LispSignal` | `.symbol`, `.data` |
| `throw` | `emacs_jni.core.LispThrow` | `.tag`, `.value` |

Both classes extend `emacs_jni.core.LispNonLocalExit`. Its fields are `.symbolOrTag` and
`.dataOrValue`.

A field that holds a symbol contains a `LispSymbol`. Use `l/->clj` to get a Clojure symbol:

```clojure
(import '[emacs_jni.core LispSignal])

(try
  (l/apply 'error "boom")
  (catch LispSignal e
    (l/->clj (.-symbol e))))
;; => error   (a Clojure symbol)
```

Here, `.data` is the Lisp list `("boom")`, so it is a `LispObject`. Read it with
`(.prin1ToString (.-data e))`.

To catch both kinds of exit, catch `LispNonLocalExit`:

```clojure
(import '[emacs_jni.core LispNonLocalExit])

(try
  (l/apply 'error "boom")
  (catch LispNonLocalExit e
    (println "Lisp exited:" (l/->clj (.-symbolOrTag e)))))
```

The future from `apply-async` completes with the same exception. `deref` and `.get` wrap it in a
`java.util.concurrent.ExecutionException`. Use `.getCause` to get it.

For a Lisp error in a Lisp function that Java calls, see
[How to pass a Lisp function to Java](pass-a-lisp-function-to-java.md#signal-an-error-from-a-callback).
