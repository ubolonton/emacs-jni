# How to pass a Lisp function to Java

This guide covers passing a Lisp function as a `jvm-call`/`jvm-new`
argument, so Java can call it back. It assumes you can already call a
plain method — see
[How to call Java from Lisp](call-java-from-lisp.md).

## Pass a lambda as an argument

```elisp
(jvm-call-sync "Calc" "callWithArg" (list (lambda (n) (* n 10)) 4))
;; => (t 40 nil)
```

Any argument that is a Lisp function — a `lambda`, or a symbol naming
one — marshals automatically into a real, callable Java object. The
Java-side method receives it as a normal object of the callback type
declared in the shim classes (see
[Interop design](../explanation/interop-design.md)). Calling it from
Java runs the Lisp function and returns the result asynchronously.

## Receive a real Java object as a callback argument

`my-calc` here is a `(jvm-new-sync "Calc" (list 100))` result, already
in scope:

```elisp
(jvm-call-sync (nth 1 my-calc) "callWithSelf"
               (list (lambda (self) (jvm-call-sync self "addToBase" (list 1)))))
```

When Java calls your Lisp function with a real object argument (not a
primitive), that argument arrives already wrapped and ready to pass
back into another `jvm-call`. You do not need to unwrap anything
yourself.

## Signal an error from inside a callback

```elisp
(jvm-call-sync "Calc" "callWithArg"
               (list (lambda (_n) (error "something went wrong")) 4))
```

A Lisp error raised inside the callback propagates back to the Java
caller as a real exception, not a silently dropped call. See
[How to handle errors and timeouts](handle-errors-and-timeouts.md).

## Know when the callback stops working

Each wrapped Lisp function stays usable for as long as the Java-side
proxy object that wraps it is reachable. Once that proxy is garbage
collected on the JVM side, the Lisp-side bookkeeping for it is dropped
too. If your Java code discards the proxy object right after the
call — the common case, as in `Calc.callWithArg` — the wrapped function
does not stay callable past that one call.

## Avoid a callback-table leak

If a `jvm-call`/`jvm-new` call that includes a Lisp-function argument
fails before the Java-side proxy is actually constructed — for
example, because the target class or method does not exist — the
Lisp-side bookkeeping entry for that function is not cleaned up. Each
attempt registers a new entry, even if you pass the same function
value each time. This is a known, low-severity gap: bounded at
prototype scale, and only hit on a failed call. See
[Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md).
Avoid retrying a failing call with a Lisp-function argument in a tight
loop, since each retry adds one more entry that never clears.
