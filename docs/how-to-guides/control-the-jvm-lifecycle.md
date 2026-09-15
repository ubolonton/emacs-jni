# How to control the JVM lifecycle

This guide assumes you can already start a JVM and call a method. See
[Your first JVM call](../tutorials/01-your-first-jvm-call.md) if you
have not done that yet. Full signatures are in the
[Lisp API reference](../reference/lisp-api.md).

## Start the JVM with a custom classpath or JVM flags

Set `jvm-classpath` and `jvm-options` before your first `jvm-start`:

```elisp
(setq jvm-classpath '("/path/to/extra/classes"))
(setq jvm-options '("-Xrs" "-Xmx512m"))
(jvm-start-sync)
```

These two variables are only honored on a genuinely fresh start, from
`not-started`, `stopped`, `start-failed`, `stop-failed`, or `crashed`.
Resuming a `paused` JVM reuses whatever options that JVM already
started with.

Keep `"-Xrs"` in `jvm-options` unless you want the JVM to install its
own signal handlers. Without `"-Xrs"`, `C-g` can terminate Emacs
outright, because the JVM shares Emacs's process. See
[Architecture](../explanation/architecture.md) for why.

## Pause the JVM without losing its state

```elisp
(jvm-pause-sync)
;; => paused
```

Pausing detaches the worker thread but keeps the JVM alive. Loaded
jars, JVM-side objects, and static state all survive. Pausing is legal
only from `ready`.

To resume, call `jvm-start-sync` again:

```elisp
(jvm-start-sync)
;; => ready
```

This resumes the same JVM. It does not re-run JVM initialization, and
it ignores `jvm-classpath`/`jvm-options` on this call.

## Stop the JVM

```elisp
(jvm-stop-sync)
;; => stopped, or stop-failed
```

Both `stopped` and `stop-failed` mean the operation finished, not that
it hung. `jvm-stop` is best-effort: some JVM builds handle the
underlying teardown call unreliably. Restarting the JVM in the same
process after a stop is **not supported** — `jvm-start` afterward
typically returns a non-functional "zombie" VM whose calls fail with
`"JNI call failed"`. A fresh Emacs process is the only reliable reset —
see
[Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md).

If you need to release JVM resources temporarily and come back later
in the same Emacs session, use `jvm-pause` instead of `jvm-stop`. It is
not experimental, and resuming is guaranteed to work.

## Watch every status transition

```elisp
(add-hook 'jvm-status-hook
          (lambda (state) (message "JVM is now: %s" state)))
```

The hook runs on every transition, including the transient
`starting`/`pausing`/`stopping` states. Those transient states are set
the moment you call the async function, before the worker thread has
done anything.

## Diagnose a failed start

If `jvm-status` returns `start-failed`, read the log buffer:

```elisp
(with-current-buffer "*jvm-log*" (buffer-string))
```

`*jvm-log*` receives `start-failed`/`stop-failed`/`crashed` status
messages, and nothing else from most operations — jar-loading and call
failures come back directly in their return value instead. The JVM's
own diagnostic (for example, an unrecognized flag) goes to Emacs's
stderr, not into `*jvm-log*`. `*jvm-log*` tells you that something
failed. It does not always say why in detail.

## Call `jvm-start`/`jvm-pause`/`jvm-stop` from the wrong state

Each of these primitives signals `jvm-illegal-transition` if the
current status does not allow it. The corresponding `-sync` wrapper
leaves `jvm-status` unchanged when this happens, so it never gets stuck
on a transient state like `starting`. Catch the condition if you need
to handle it:

```elisp
(condition-case err
    (jvm-pause-sync)
  (jvm-illegal-transition
   (message "cannot pause from %s" (jvm-status))))
```
