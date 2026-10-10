# How to handle errors and timeouts

The bare (`-direct`) and `-await` functions signal a Lisp condition on failure. The `-async`
functions send the failure to their callback. For all condition names, see the
[Lisp API reference](../reference/lisp-api.md#error-conditions).

## Catch a failure from a blocking call

```elisp
(condition-case err
    (jvm-call "Calc" "addLong" (list 2 3))
  (jvm-error
   (message "call failed: %s" (cadr err))))
```

The `jvm-error` data is a message string. It contains one of these:

- The Java exception message and its cause chain.
- The Clojure exception message.
- `no method NAME with N argument(s) found`, if no candidate accepts the Lisp arguments.

## Catch each condition

| Condition | When |
|---|---|
| `jvm-error` | A Java or Clojure call, or a jar load, failed. |
| `jvm-timeout` | An `-await` call did not complete before its timeout. |
| `jvm-invalid-argument` | An argument has no marshalling rule, for example a vector. |
| `jvm-illegal-transition` | The JVM status does not allow the operation. |

`jvm-invalid-argument` occurs before the call starts, for all variants. `jvm-illegal-transition`
occurs for `jvm-start`, `jvm-pause`, and `jvm-stop` from a wrong status. It also occurs for a
call or a jar load when the JVM is not `ready`.

```elisp
(condition-case err
    (jvm-call-await "SlowClass" "slowMethod" nil 5)
  (jvm-timeout          (message "timed out after 5 seconds"))
  (jvm-error            (message "call failed: %s" (cadr err)))
  (jvm-invalid-argument (message "bad argument: %S" err)))
```

## Handle a failure from an async call

The callback receives `(OK RESULT ERR)`. For `jvm-load-jar-async`, it receives `(OK ERR)`. If
`OK` is `nil`, `ERR` is the message string.

```elisp
(jvm-call-async "Calc" "addLong" (list 2 3)
                (lambda (ok result err)
                  (if ok
                      (message "got: %s" result)
                    (message "call failed: %s" err))))
```

An argument with no marshalling rule signals `jvm-invalid-argument` from `jvm-call-async`. The
callback does not run.

## Give a call more time

Use the `-await` variant. Its last argument is the timeout in seconds. The default is 30.

```elisp
(jvm-call-await "SlowClass" "slowMethod" nil 120)
```

The bare (`-direct`) and `-async` functions take no timeout.

## Tell a Java exception from a marshalling failure

Both use `jvm-error`, or `ERR` in an `-async` callback. Read the message:

- A marshalling failure gives `no method NAME with N argument(s) found`, or
  `no constructor with N argument(s) found`. The message does not name the parameter type.
- A Java exception contains one `CLASS: MESSAGE` line for each exception in its cause chain.

## Handle an error in a Lisp callback

See
[How to pass a Lisp function to Java](pass-a-lisp-function-to-java.md#signal-an-error-from-a-callback).
