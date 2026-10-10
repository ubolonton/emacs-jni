# How to call Java from Lisp

This guide uses `jvm-call` and `jvm-new`. It assumes that the JVM is `ready` and that the class
is loaded. See [How to load a jar](load-a-jar.md). For all marshalling rules, see the
[Lisp API reference](../reference/lisp-api.md#argument-and-return-value-marshalling).

## Call a static method

```elisp
(jvm-call "java.lang.Math" "max" (list 3 7))
;; => 7
```

`TARGET` is a class name. `MEMBER` is the method name. `ARGS` is a list of Lisp values.
`jvm-call` returns the result. It signals `jvm-error` on failure.

## Call an instance method

Construct an object, then use it as `TARGET`:

```elisp
(let ((list-object (jvm-new "java.util.ArrayList" nil)))
  (jvm-call list-object "add" (list "hello"))
  (jvm-call list-object "size" nil))
;; => 1
```

## Construct an object

```elisp
(jvm-new "java.lang.StringBuilder" (list "hi"))
;; => #<user-ptr ...>
```

`jvm-new` returns a wrapped JVM object. It is `jvm-call` with `MEMBER` set to `"<init>"`.

## Pass a number

```elisp
(jvm-call "Calc" "addLong" (list 2 3))
;; => 5
```

An integer converts to the numeric type that the parameter declares: `int`, `short`, `byte`,
`long`, `float`, `double`, or `BigInteger`. If a value does not fit an `int`, `short`, or `byte`
parameter, the module skips that candidate. A Lisp float converts to `float`, `double`, or `Object` (as
a `Double`). Boxed parameter types, such as `java.lang.Long`, do not accept Lisp numbers.

## Pass a boolean

```elisp
(jvm-call "Calc" "describe" (list t))
;; => "yes"

(jvm-call "Calc" "describe" (list nil))
;; => "no"
```

`t` converts to `true`. `nil` converts to `false` for a `boolean` or `Boolean` parameter. For any
other parameter type, `nil` converts to `null`.

## Pass a wrapped JVM object

Pass the object that an earlier `jvm-new` or `jvm-call` returned:

```elisp
(let ((calc (jvm-new "Calc" (list 100))))
  (jvm-call "Calc" "isLispCallable" (list calc)))
;; => nil
```

## Set a timeout

Use `jvm-call-await` or `jvm-new-await`. The last argument is the timeout in seconds. The default
is 30.

```elisp
(jvm-call-await "SlowClass" "slowMethod" nil 120)
```

After the timeout, the call signals `jvm-timeout`. The bare `jvm-call` has no timeout.

## Call without blocking Emacs

```elisp
(jvm-call-async "Calc" "addLong" (list 2 3)
                (lambda (ok result err)
                  (if ok
                      (message "result: %s" result)
                    (message "call failed: %s" err))))
```

`jvm-call-async` and `jvm-new-async` return a request ID immediately. The callback runs on the
Emacs main thread after the worker thread completes the call.

## Select one of two overloaded methods

The module tries same-arity candidates in an unspecified order. It uses the first candidate that
accepts the arguments. It does not select the most specific one. For example:

```elisp
(jvm-call "java.lang.String" "valueOf" '(42))
;; => "42.0"
```

A `float` or `double` overload accepted `42` first. To select one overload, call a method with a
unique name or arity. See
[Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md).
