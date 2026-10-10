# How to pass a Lisp function to Java

This guide passes a Lisp function as an argument, so that Java can call it. It assumes that you
can call a Java method. See [How to call Java from Lisp](call-java-from-lisp.md).

The examples use `Calc` from `test/fixtures/Calc.java`. Java receives the function as an
`emacs_jni.core.LispCallable`. Its `call` method returns a `CompletableFuture`.

## Pass a lambda

```elisp
(jvm-call-await "Calc" "callWithArg" (list (lambda (n) (* n 10)) 4))
;; => 40
```

Use `jvm-call-await` or `jvm-call-async` if the Java method waits for the Lisp result. The Lisp
function runs on the Emacs main thread. A bare `jvm-call` blocks that thread. The Java method then
waits until its own timeout, and Emacs stays blocked. See
[Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md#a-lispfunction-result-is-not-available-inside-a-direct-call).

## Pass a named function

Pass the function object, not the symbol:

```elisp
(defun my-times-ten (n) (* n 10))

(jvm-call-await "Calc" "callWithArg" (list (symbol-function 'my-times-ten) 4))
;; => 40
```

A symbol, such as `'my-times-ten` or `#'my-times-ten`, goes to Java as a `LispSymbol`. Java cannot
call a `LispSymbol`. If the name is an alias made with `defalias`, use `indirect-function` instead
of `symbol-function`.

A lambda, closure, byte-compiled function, or built-in function (subr) goes to Java as a callable.

## Receive a Java object in a callback

```elisp
(let ((calc (jvm-new "Calc" (list 100))))
  (jvm-call-await calc "callWithSelf"
                  (list (lambda (self) (jvm-call self "addToBase" (list 1))))))
;; => 101
```

A Java object argument arrives as a wrapped JVM object. Pass it to `jvm-call` without change.

## Signal an error from a callback

```elisp
(jvm-call-await "Calc" "describeCallbackError"
                (list (lambda (_n) (error "Something went wrong")) 4))
;; => "error"
```

A Lisp error completes the future exceptionally:

| Lisp exit | Java exception |
|---|---|
| `signal`, `error` | `emacs_jni.core.LispSignal` |
| `throw` | `emacs_jni.core.LispThrow` |

Both classes extend `emacs_jni.core.LispNonLocalExit`. `CompletableFuture.get` wraps the exception
in an `ExecutionException`. Read it with `getCause`. `Calc.describeCallbackError` does this.

For how long Java can call a function, see
[Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md).
