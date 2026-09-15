# How to call Java from Lisp

This guide covers `jvm-call`/`jvm-call-await` and `jvm-new`/
`jvm-new-await`. It assumes the JVM is `ready` and the relevant jar or
class is already loaded. See [How to load a jar](load-a-jar.md). Full
marshalling rules are in the
[Lisp API reference](../reference/lisp-api.md#argument-and-return-value-marshalling).

## Call a static method

```elisp
(jvm-call-await "java.lang.Math" "max" (list 3 7))
;; => (t 7 nil)
```

`TARGET` is a class name string. `MEMBER` is the method name. `ARGS`
is a list of Lisp values.

## Call an instance method

Construct an object first, then pass it as `TARGET`:

```elisp
(let ((obj (jvm-new-await "java.util.ArrayList" nil)))
  (jvm-call-await (nth 1 obj) "add" (list "hello"))
  (jvm-call-await (nth 1 obj) "size" nil))
;; => (t 1 nil)
```

`(nth 1 obj)` pulls the wrapped JVM object out of `jvm-new-await`'s
`(OK RESULT ERR)` result.

## Construct an object

```elisp
(jvm-new-await "java.lang.StringBuilder" (list "hi"))
;; => (t #<user-ptr ...> nil)
```

`jvm-new`/`jvm-new-await` are `jvm-call`/`jvm-call-await` with `MEMBER`
fixed to the constructor's internal name, `"<init>"`. Everything else
about them, including argument marshalling and error handling, is
identical.

## Pass a number

```elisp
(jvm-call-await "Calc" "addLong" (list 2 3))
;; => (t 5 nil)
```

An integer marshals to whichever numeric type the target parameter
declares (`int`, `short`, `byte`, `long`, `float`, `double`, or a boxed
`java.math.BigInteger`), and this module checks that the value fits —
a value too large for the target type is a marshalling error, not a
silent truncation. A float marshals to `float`, `double`, or a boxed
`java.lang.Double`.

## Pass a boolean

```elisp
(jvm-call-await "Calc" "describe" (list t))
;; => (t "yes" nil)

(jvm-call-await "Calc" "describe" (list nil))
;; => (t "no" nil)
```

`t` marshals to Java `true`. `nil` marshals to Java `false` when the
target parameter is `boolean`, and to a Java `null` otherwise — the
same Lisp value means two different things in Java, depending on what
the method expects.

## Pass a wrapped JVM object

`obj` here is the `RESULT` part of an earlier `jvm-call-await`/
`jvm-new-await` result, for example `(nth 1 (jvm-new-await "Calc" (list 100)))`:

```elisp
(jvm-call-await "Calc" "isLispCallable" (list obj))
```

Pass the wrapped object exactly as it came back. It marshals to its
real Java type.

## Set a timeout longer than the default 30 seconds

```elisp
(jvm-call-await "SlowClass" "slowMethod" nil 120)
```

The fourth argument to `jvm-call-await` (third for `jvm-new-await`) is
the timeout, in seconds.

## Call without blocking

```elisp
(jvm-call-async "Calc" "addLong" (list 2 3)
                (lambda (ok result err)
                  (if ok
                      (message "result: %s" result)
                    (message "call failed: %s" err))))
```

`jvm-call-async`/`jvm-new-async` return a request ID immediately. The callback runs
later, on the same Lisp thread that is running Emacs's event loop, once
the JVM worker thread finishes the call.

## Choose between two overloaded methods

Overload resolution picks the first same-arity candidate that accepts
your argument types, and falls through to the next one only if
marshalling that candidate's parameters fails. If two overloads share
the same arity and both could accept your argument (for example, one
takes `long` and the other takes `String`, and you are passing a
number), the resolution is not guaranteed to pick the one you expect.
If this matters, give the two overloads different arities, or route
through a differently named method. See
[Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md).
