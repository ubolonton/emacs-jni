# How to handle errors and timeouts

This module reports failure two different ways, depending on which
variant you call: the synchronous forms signal a Lisp condition, the
asynchronous form reports to its callback. This guide shows both. Full
condition names are in the
[Lisp API reference](../reference/lisp-api.md#error-conditions).

## Catch a failure from a synchronous call

The synchronous forms — the bare name (`-direct`) and the `-await`
variant — return the value on success and signal a Lisp condition on
failure. Catch it the normal way:

```elisp
(condition-case err
    (jvm-call "Calc" "addLong" (list 2 3))
  (jvm-error
   (message "call failed: %s" (cadr err))))
```

`jvm-error` carries a descriptive string: the underlying Java
exception's message and cause chain if the failure was on the Java
side, a Clojure exception's message, or a marshalling error if a Lisp
argument did not fit the target Java type.

## The conditions you might see

- `jvm-error` — a Java/Clojure call failed. From the `-direct` and
  `-await` forms of `jvm-call`/`jvm-new`/`jvm-load-jar` and the
  `jvm-clj-*` functions.
- `jvm-timeout` — an `-await` call did not finish before its timeout.
- `jvm-invalid-argument` — a Lisp argument to `jvm-call`/`jvm-new` has
  no marshalling rule at all (e.g. a vector). Signaled up front, for
  every variant, before the call is dispatched.
- `jvm-illegal-transition` — `jvm-start`/`jvm-pause`/`jvm-stop`, or a
  `-direct` call with no running JVM, invoked from a status that does
  not allow it.

Catch several at once the normal way:

```elisp
(condition-case err
    (jvm-call-await "SlowClass" "slowMethod" nil 5)
  (jvm-timeout          (message "timed out after 5 seconds"))
  (jvm-error            (message "call failed: %s" (cadr err)))
  (jvm-invalid-argument (message "bad argument: %S" err)))
```

## Handle a failure from an asynchronous call

The `-async` forms do not signal — they report to their callback. The
callback receives `(OK RESULT ERR)` (`(OK ERR)` for
`jvm-load-jar-async`); check `OK`, and read the `ERR` string when it is
nil:

```elisp
(jvm-call-async "Calc" "addLong" (list 2 3)
                (lambda (ok result err)
                  (if ok
                      (message "got: %s" result)
                    (message "call failed: %s" err))))
```

(An unmarshallable argument still signals `jvm-invalid-argument`
synchronously, from the `jvm-call-async` call itself, before any
callback.)

## Give a call more time before it times out

Only the `-await` (and `-async`) forms take a timeout; it is the last
argument, in seconds, and defaults to 30. The bare `-direct` call runs
to completion with no timeout.

```elisp
(jvm-call-await "SlowClass" "slowMethod" nil 120)
```

## Tell a Java exception apart from a marshalling failure

Both arrive the same way — a string carried by `jvm-error` (or in the
`-async` callback's `ERR`). There is no separate condition for "the
Java method threw" versus "an argument that had a rule did not fit."
Read the string: a marshalling failure names the offending argument and
the expected type; a Java exception carries the exception's own class
name and message. A Lisp value with no rule at all is the exception —
it signals `jvm-invalid-argument` up front instead.

## Handle an error raised inside a Lisp callback

See
[How to pass a Lisp function to Java](pass-a-lisp-function-to-java.md#signal-an-error-from-inside-a-callback).
An `error` signaled inside a Lisp function passed to Java propagates to
the Java caller as a real exception on that side, not a hang or a
dropped call.
