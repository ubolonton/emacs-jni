# Architecture

This page explains how emacs-jni puts a JVM inside the Emacs process, and why it has this
shape. For what each function does, see the [Lisp API reference](../reference/lisp-api.md).
For the Rust-level design, see [Implementation](implementation.md).

## Two paths into the JVM

emacs-jni gives Lisp two paths into the JVM: the direct path and the worker path. Bare names
(`jvm-call`, `jvm-new`, `jvm-load-jar`, all `jvm-clj-*` functions) use the direct path.

The **direct path** (`-direct`) runs the call on the main thread. The main thread attaches to
the JVM on its first direct call and stays attached. So `Thread.currentThread()` stays the same
across direct calls, and Java `ThreadLocal` values persist. Clojure's `*ns*` does not persist,
because each eval binds it for that eval only. A direct call from another Lisp thread attaches for
that call only.

A direct call has no thread hop and no queue, so it is the fastest path. Its cost is that it
blocks Emacs until it returns. It holds the Emacs global lock, so it also blocks every other Lisp
thread.

The **worker path** serves `-await` and `-async`. Lisp sends a command to the worker thread and
returns at once. `-async` gives the result to a callback. `-await` waits for the result in an
`accept-process-output` loop, with a timeout.

Both paths exist because they solve different problems. The direct path gives low latency and a
stable JVM thread. The worker path keeps long Java work from freezing Emacs.

The worker thread also owns the JVM lifecycle. It creates the JVM at `jvm-start`, detaches from
it at `jvm-pause`, and destroys it at `jvm-stop`. These lifecycle commands have no `-direct`
form. Their bare names use `-await`.

## How results return on the worker path

Lisp cannot block on a read from the JVM, because a blocking read freezes Emacs. So results come
back as an event that Emacs already knows how to schedule: output from a pipe process.

Every message from the JVM side goes into one typed queue in the module. Examples are a finished
call, a call into a Lisp function, a log line, a status change, and a Clojure request for a Lisp
value. After each push, the sender writes one wakeup byte to the pipe. The byte has no payload.
The pipe's process filter runs on the main thread and empties the whole queue.

The queue carries the data, and the pipe only schedules the drain. So two wakeups that arrive
together cause no problem, because one drain empties the queue. The filter runs when Emacs is
idle, or inside the `accept-process-output` loop of an `-await` function.

## A call gets its own thread, not the worker thread

On the worker path, a `-async` or `-await` call does not run on the worker thread. The worker
starts a new OS thread for each call. That thread attaches to the JVM, makes the reflective call,
queues the result, and exits. So one slow call does not stop other calls, or lifecycle commands.

This has three costs:

- Each call pays for a thread start and a JNI attach.
- Java `ThreadLocal` values do not persist from one call to the next.
- A Rust panic in that thread is not caught. The `-await` caller gets `jvm-timeout`, with no
  other diagnostic.

See [Limitations and trade-offs](limitations-and-tradeoffs.md) for the consequences.

## Why jar loading goes through reflection

JNI's `FindClass` resolves a class through the classloader of the calling code. On a thread
that JNI attached, that is the system classloader. A jar added after JVM start is not on the
system classpath, so `FindClass` cannot see its classes.

So emacs-jni puts every loaded jar in one shared `URLClassLoader`. It adds each jar with
`addURL`, through reflection, because that method is protected. It resolves class names with the
loader's `loadClass` method, not `FindClass`. The loader asks its parent first, so JDK classes
also resolve.

Both paths use the same loader. A class loaded through a direct call is the same class on the
worker path. Because all jars share one loader, classes in one jar can see classes in another
jar.

## Why each call sets the context classloader

A thread that JNI attached has a `null` context classloader. Reflection through the shared loader
does not need it. Clojure does: it loads `.clj` source files as classpath resources through the
context classloader. With a `null` context classloader, the first use of Clojure fails with
`ExceptionInInitializerError`.

So, after the first jar load, each call sets the current thread's context classloader to the
shared loader before it does reflective work. This applies on both paths. The cost is one extra
JNI call per call.

## Why `-Xrs` is the default JVM option

A JVM usually installs handlers for `SIGINT`, `SIGTERM`, `SIGHUP`, and `SIGQUIT`. Inside
the Emacs process, these handlers compete with the handlers of Emacs. For example, `C-g` can
kill Emacs instead of stopping the current command. `-Xrs` stops the JVM from installing these
handlers.

The cost is that the JVM's signal features do not work, for example a thread dump on `SIGQUIT`.
Remove `-Xrs` from `jvm-options` only if you need them.

## Why Clojure calls into Lisp run on the Emacs main thread

Clojure code can hold and use Lisp values through `emacs-jni.clj`. The operations are `apply`,
`intern`, `type-of`, `eq`, `is-not-nil?`, and `LispObject.prin1ToString`. Each one reads or
creates Lisp values. Emacs allows that only on the main thread, during a module call. A JVM
thread cannot do it.

What happens depends on the thread that runs the Clojure code:

- **Inside a direct call.** The Clojure code runs on the main thread, inside the module call of
  `jvm-call-direct`. The request runs at once, on that thread. It uses no queue, so it cannot
  deadlock.
- **On any other thread.** Examples are an `-await` call thread, an nREPL thread, or a thread that
  Clojure starts. The request goes into the queue, and the JVM thread waits. Emacs runs the
  request when it next drains the queue: when idle, or inside an `-await` wait loop.

The queue cannot serve a direct call. During a direct call, the main thread runs the Java code,
so it cannot drain the queue. A queued request would wait for a drain that cannot happen. So the
direct path runs the request at once, inside the direct call.

A queued request waits until Emacs is free. `intern`, `type-of`, `eq`, `is-not-nil?`, and
`prin1ToString` wait up to 30 seconds. `apply` and the callables from `require-symbol`,
`require-function`, `refer-symbols`, and `refer-functions` wait with no timeout. See
[Limitations and trade-offs](limitations-and-tradeoffs.md).

A call to a `LispFunction` (a Lisp function passed to Java) follows the same rules. It returns a
`CompletableFuture`. On the main thread inside a direct call, the Lisp function runs at once, and
the future is already complete. On any other thread, the call goes through the queue. If a direct
call waits for that thread, the wait ends only at the Java timeout, or never if it has none.

A Lisp `signal` or `throw` in such a request reaches Java as a `LispSignal` or `LispThrow`. If it
unwinds back out of a `jvm-call-direct`, Lisp gets the original signal or throw again.

## Why Lisp functions and JVM objects need bidirectional GC notification

Three kinds of object cross between the two runtimes:

- A Lisp function passed to Java becomes a `LispFunction`.
- A JVM object returned to Lisp becomes a wrapped JVM object.
- A Lisp value held by Clojure becomes a `LispObject`.

Each one keeps an object alive in the other runtime. Neither garbage collector can see into the
other heap. So each wrapper tells the other side when its collector frees it:

- When the JVM collects a `LispFunction`, a `Cleaner` tells Emacs to remove the Lisp function from
  its callback table.
- When Emacs collects a wrapped JVM object, the module deletes its JVM reference on the worker
  thread.
- When the JVM collects a `LispObject`, a `Cleaner` queues a request to free the Lisp value. The
  main thread frees it, because only the main thread can.

Without these notifications, each side would keep the other side's objects alive until the JVM
stops. This design assumes that no reference cycle spans both runtimes. See
[Interop design](interop-design.md#the-no-cycles-assumption).
