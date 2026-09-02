# Architecture

This page discusses how emacs-jni fits a JVM inside an Emacs process,
and why it is built this way. For what each function does, see the
[Lisp API reference](../reference/lisp-api.md). For steps, see the
how-to guides.

## One worker thread owns the JVM

A JVM, once created inside a process, expects a fairly stable
relationship with the OS threads that touch it. Emacs's own Lisp
execution model — mostly single-threaded, cooperative, with an event
loop — does not fit a JVM well if every Lisp call has to reach into the
JVM directly.

emacs-jni resolves this by giving the JVM to one dedicated OS thread,
for the JVM's whole lifetime. That thread is created once, at
`jvm-start`, and is the only thread that ever calls `JNI_CreateJavaVM`
or the corresponding attach/detach calls for the JVM's main
association. Lisp never touches the JVM directly. Instead, Lisp sends
a command, and the worker thread does the JVM-side work.

This buys a JVM lifecycle that is independent of Emacs's own threading
quirks, at the cost of every operation needing a round trip through a
channel instead of a direct function call. `jvm-status` is the one
exception — it reads a local Lisp variable, not the worker thread, so
it needs no round trip.

## Two channels, two directions

Lisp talks to the worker thread through an `mpsc` command channel, on
the Rust side. Each Lisp-facing primitive (`jvm--start`, `jvm--pause`,
`jvm--call`, and so on) sends one command and returns immediately —
this is why the plain (non-`-sync`) functions are async by default,
not a fetched-in-later design choice.

The worker thread talks back to Lisp through a different mechanism: an
Emacs pipe process, backed by `Env::open_channel`, carrying
newline-terminated S-expressions. `jvm--process-filter` in
`elisp/emacs-jni.el` accumulates bytes until it sees a complete line,
reads it as a Lisp form, and dispatches on its head symbol:
`status`, `log`, `load-jar-done`, `call-done`, `invoke`, or `release`.

Two separate channels, rather than one bidirectional one, follows from
which side can block. The worker thread can block on the `mpsc`
channel between commands, with no ill effect. Lisp cannot block
waiting on the JVM without freezing Emacs — so the worker-to-Lisp
direction has to arrive as an event the existing Emacs process filter
mechanism already knows how to schedule, not as a blocking read.

## A call gets its own thread, not the worker thread

Once the JVM exists, `jvm-call`/`jvm-new` do not run on the worker
thread itself. Each call spawns a fresh OS thread, attaches it to the
JVM, does the reflective call, and lets the thread finish. This keeps
one slow or blocked call from stalling every other call in flight —
the tradeoff is that each call needs its own JNI attach, and anything
that depends on JVM-side thread-local state does not carry over from
one call to the next.

That tradeoff is exactly what causes Clojure's `*ns*` to reset between
separate `jvm-clojure-eval-string` calls, and it is why the
context-classloader fix (below) had to apply per spawned thread, not
once at JVM start. See
[Limitations and trade-offs](limitations-and-tradeoffs.md) for the
practical consequence.

## Why jar loading goes through reflection

JNI's `FindClass` resolves a class using the calling thread's defining
classloader, not the JVM's system classloader. A class loaded from a
jar added after JVM start is invisible to `FindClass` from an
arbitrary attached thread. emacs-jni works around this with one shared
`URLClassLoader`, and reaches every class — jar-loaded or not — through
that loader's `loadClass` method, called reflectively, instead of
`FindClass`. Every loaded jar shares this one loader, which is also
why classes across separately loaded jars can already see each other.

## Why a freshly spawned thread needed a context classloader fix

A freshly attached JNI thread has a `null` context classloader.
Ordinary reflective class resolution, as `jvm-call` already used
everywhere, does not consult it. Clojure's own namespace loader does:
loading a `.clj` source file happens through classpath *resource*
resolution, which falls back through
`Thread.currentThread().getContextClassLoader()`. Left `null`, the
first Clojure `require` call failed with
`ExceptionInInitializerError` before `clojure.core` itself finished
loading.

The fix sets each spawned call-thread's context classloader to the
shared `URLClassLoader`, once, right after attaching and before any
reflective work. It is a cheap, one-time JNI call, and a no-op for
calls that never touch Clojure. This was found empirically, during a
pre-planning spike, not predicted from reading the JNI specification —
see `docs/superpowers/specs/2026-09-02-clojure-nrepl-design.md`'s
"Feasibility" section for how it was diagnosed.

## Why `-Xrs` is the default JVM option

A JVM normally installs its own handlers for `SIGINT`, `SIGTERM`,
`SIGHUP`, and `SIGQUIT`, to support `Runtime.addShutdownHook` and a
`SIGQUIT` thread dump. Because the JVM lives inside Emacs's own
process, those handlers would compete with Emacs's own signal
handling — in particular, `C-g` could terminate Emacs outright instead
of just interrupting the current command. `-Xrs` stops the JVM from
installing those handlers at all. `jvm-stop` still runs shutdown hooks
correctly with `-Xrs` set, because it tears the JVM down through the
JNI API directly, not through a signal.

## Why Lisp functions and JVM objects need bidirectional GC notification

A Lisp function passed into a `jvm-call` becomes a real Java object (a
`LispCallable`/`LispFunction` proxy, from the shim classes described in
[Interop design](interop-design.md)). A JVM object returned to Lisp
becomes a wrapped Lisp value (a `user-ptr`). Each side of that
wrapping is a live reference the other runtime's garbage collector
cannot see on its own.

Both directions notify the other side on collection: a JVM-side
`Cleaner` sends a `release` message when a `LispFunction` proxy is
collected, and Emacs's own GC, running on a `JvmHandle` user-ptr, sends
a release the other way. Without this, the object graph on either side
would keep the other side's memory alive indefinitely, or would only
release it when the whole JVM stops. This design assumes the
cross-runtime object graph has no cycles — see
[Interop design](interop-design.md) for what that assumption costs.
