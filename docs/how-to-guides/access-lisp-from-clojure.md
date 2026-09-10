# How to access Emacs Lisp from Clojure

This guide covers the other direction of interop: Clojure code that
holds and manipulates Emacs Lisp values directly. It uses the
`jvm-clj.el` library and the `emacs-jni.clj` Clojure namespace.
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
`emacs-jni.clj`. It signals an error if any step fails — for
example, if no Clojure jar is on `jvm-clj-classpath`.

To start an nREPL server with the namespace already loaded, use
`jvm-clj-nrepl-start` in place of `jvm-nrepl-start`.

## Alias the namespace

`emacs-jni.clj` defines `intern`, `apply`, and `eq`, which shadow
`clojure.core`. Alias it rather than referring the names in:

```clojure
(require '[emacs-jni.clj :as l])
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

## Import a Lisp name as a callable

Calling through `apply` (`(l/apply 'message "hi")`) works for any
name, but reads awkwardly for code that calls the same Lisp function
many times. `require-symbol`/`refer-symbols` and `require-function`/
`refer-functions` import a Lisp name as a plain Clojure callable
instead:

```clojure
((l/require-symbol 'message) "hi")   ; late-bound: funcalls `message` each call

(l/refer-symbols message)
(message "hi")                       ; now calls Lisp's `message`, like any Clojure fn
```

`require-symbol` returns a callable that funcalls the named Lisp
symbol fresh on every call — late binding. If Lisp code later
redefines `message` (`defun`/`fset`), the next call picks up the new
definition. `refer-symbols` is the same thing as a macro: it `def`s a
var per symbol, named after the symbol, in the current namespace.

`require-function`/`refer-functions` bind early instead: they resolve
`indirect-function` once, at import time, and keep calling that one
function object, so a later redefinition of the Lisp symbol has no
effect on the imported callable.

```clojure
(l/refer-functions message)          ; resolves `message`'s function object now
(message "hi")                       ; always calls that same function object
```

Use `require-symbol`/`refer-symbols` when Lisp code may redefine the
target later (interactive development, `advice-add`, and so on). Use
`require-function`/`refer-functions` to pin down one specific
definition.

`refer-symbols`/`refer-functions` only work for Lisp names that are
also legal Clojure `def` symbols. For a name that is not (for example,
one containing a character illegal in a Clojure symbol), use
`require-symbol`/`require-function` with a string instead:

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

A Lisp non-local exit during `apply`/`apply-async` — `error`/`signal`,
or `throw` — surfaces as a `clojure.lang.ExceptionInfo` (`ex-info`),
not a plain exception:

```clojure
(try
  (l/apply (l/intern "error") "boom")
  (catch clojure.lang.ExceptionInfo e
    (ex-data e)))
;; => {:type :emacs-jni.clj/lisp-signal, :symbol error, :data #object[...]}
```

`:symbol` converts to a Clojure symbol (interned), same as any other
result. `:data` here is the Lisp error-data list `("boom")` — a list,
not a scalar, so it stays an opaque `LispObject` (read it with
`(.prin1ToString (:data (ex-data e)))` → `"(\"boom\")"`; see
[Read an opaque value's printed form](#read-an-opaque-values-printed-form)
above).

`ex-data` carries which case it is, and the exit's payload, already
converted the same way as any other result:

| Exit | `(ex-data e)` |
|---|---|
| `signal` (`error`, `signal`) | `{:type :emacs-jni.clj/lisp-signal :symbol <sym> :data <data>}` |
| `throw` | `{:type :emacs-jni.clj/lisp-throw :tag <tag> :value <value>}` |

`(:symbol (ex-data e))` reads the signaled condition's symbol (for
example `error`, or a custom condition from `define-error`); `:data`
is its data list. `lisp-signal?`/`lisp-throw?` test which case an
exception is, without pulling `ex-data` apart yourself:

```clojure
(try
  (l/apply (l/intern "error") "boom")
  (catch clojure.lang.ExceptionInfo e
    (if (l/lisp-signal? e)
      (println "Lisp signaled:" (:symbol (ex-data e)))
      (throw e))))
```

`apply-async`'s `CompletableFuture` completes exceptionally with the
same `ex-info` as its completion exception, but `deref` (`.get()`)
wraps it in a `java.util.concurrent.ExecutionException` — read
`(.getCause e)` for the `ex-info`, or use `apply` to get the `ex-info`
directly. See
[How to pass a Lisp function to Java](pass-a-lisp-function-to-java.md)
for the mirror-image case, a Lisp error raised inside a callback Java
calls.

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
