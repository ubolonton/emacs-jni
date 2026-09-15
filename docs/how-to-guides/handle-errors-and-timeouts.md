# How to handle errors and timeouts

This module reports failure two different ways, depending on when the
failure happens. This guide shows how to handle both. Full condition
names are in the
[Lisp API reference](../reference/lisp-api.md#error-conditions).

## Check the result of a `-sync` call before using it

`jvm-call-await`, `jvm-new-await`, `jvm-clj-eval-string-await`,
`jvm-clj-nrepl-start-await`, and similar functions return `(OK RESULT ERR)`.
`jvm-load-jar-await` returns the shorter `(OK . ERR)`. Always check `OK`
first:

```elisp
(let ((result (jvm-call-await "Calc" "addLong" (list 2 3))))
  (if (car result)
      (message "got: %s" (nth 1 result))
    (message "call failed: %s" (nth 2 result))))
```

A failed call still returns normally. It does not signal a Lisp error.
`ERR` is a descriptive string: the underlying Java exception's message
and cause chain, if the failure happened on the Java side, or a
marshalling error, if a Lisp argument did not fit the target Java
type.

## Catch a signaled condition

Three situations signal a real Lisp condition instead of returning
`(nil ... ERR)`:

- A `-sync` call that does not finish before its timeout signals
  `jvm-timeout`.
- An unmarshallable argument to `jvm-call`/`jvm-new` (a Lisp value this
  module has no rule for) signals `jvm-invalid-argument`.
- `jvm-start`/`jvm-pause`/`jvm-stop`, called from a status that does
  not allow it, signals `jvm-illegal-transition`.

Catch these the normal way:

```elisp
(condition-case err
    (jvm-call-await "SlowClass" "slowMethod" nil 5)
  (jvm-timeout
   (message "timed out after 5 seconds"))
  (jvm-invalid-argument
   (message "bad argument: %S" err)))
```

## Give a call more time before it times out

Every `-sync` function's last argument is a timeout, in seconds. It
defaults to 30:

```elisp
(jvm-call-await "SlowClass" "slowMethod" nil 120)
```

## Tell a Java exception apart from a marshalling failure

Both come back the same way, as a string in `ERR`. There is no
separate condition type for "the Java method threw" versus "one of
your arguments did not fit." Read the string instead. A marshalling
failure names the offending argument and the expected type. A Java
exception carries the exception's own class name and message.

## Handle an error raised inside a Lisp callback

See
[How to pass a Lisp function to Java](pass-a-lisp-function-to-java.md#signal-an-error-from-inside-a-callback).
An `error` signaled inside a Lisp function passed to Java propagates to
the Java caller as a real exception on that side, not a hang or a
dropped call.
