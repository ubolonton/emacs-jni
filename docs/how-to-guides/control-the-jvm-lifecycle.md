# How to control the JVM lifecycle

This guide assumes that you can start a JVM and call a method. See
[Your first JVM call](../tutorials/01-your-first-jvm-call.md). For all signatures, see the
[Lisp API reference](../reference/lisp-api.md).

## Start the JVM with a custom classpath or JVM flags

Set `jvm-classpath` and `jvm-options` before `jvm-start`:

```elisp
(setq jvm-classpath '("/path/to/extra/classes"))
(setq jvm-options '("-Xrs" "-Xmx512m"))
(jvm-start)
;; => ready
```

`jvm-start` reads these variables only when it creates a JVM. A resume from `paused` ignores them.

Keep `"-Xrs"` in `jvm-options`. Without it, the JVM installs its own signal handlers, and `C-g`
can kill Emacs. See [Architecture](../explanation/architecture.md).

## Pause the JVM and keep its state

```elisp
(jvm-pause)
;; => paused
```

The worker thread detaches, and the JVM stays alive. Loaded jars, JVM objects, and static state
stay. You can pause only from `ready`.

To resume, call `jvm-start` again:

```elisp
(jvm-start)
;; => ready
```

`jvm-start` resumes the same JVM. It does not initialize the JVM again.

## Stop the JVM

```elisp
(jvm-stop)
;; => stopped
```

The result is `stopped` or `stop-failed`. A new `jvm-start` in the same Emacs process usually
gives a JVM whose calls fail. To get a new JVM, start a new Emacs process. To release the JVM for
a time and use it again later, use `jvm-pause`. See
[Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md).

## Run code on each status change

```elisp
(add-hook 'jvm-status-hook
          (lambda (state) (message "JVM is now: %s" state)))
```

The hook runs on each status change. This includes the transient statuses `starting`, `pausing`,
and `stopping`. Lisp sets a transient status when you call the function, before the worker
thread starts the operation.

## Find why the JVM did not start

If `jvm-status` returns `start-failed`, read the log buffer:

```elisp
(with-current-buffer "*jvm-log*" (buffer-string))
```

`*jvm-log*` gets the `start-failed`, `stop-failed`, and `crashed` messages. The JVM writes its own
diagnostics, for example about a flag that it does not know, to the Emacs stderr.

## Handle a call from a wrong status

`jvm-start`, `jvm-pause`, and `jvm-stop` signal `jvm-illegal-transition` if the current status
does not allow the operation. The function then restores the previous status. `jvm-status-hook`
runs for the transient status and again for the restored status.

```elisp
(condition-case nil
    (jvm-pause)
  (jvm-illegal-transition
   (message "cannot pause from %s" (jvm-status))))
```
