# Limitations and trade-offs

This page discusses why emacs-jni's known limitations exist, and what
each one costs. For the bare facts, see the
[Lisp API reference](../reference/lisp-api.md). For workarounds, see
the relevant how-to guide.

## `jvm-stop` is best-effort

`jvm-stop` tears the JVM down through `JNI_DestroyJavaVM`. Some JVM
builds handle this call unreliably — a JVM is not designed with the
expectation that an embedding host will ask it to shut down and then
keep running itself. Rather than have `jvm-stop` hang indefinitely
waiting for a guarantee the JVM specification does not make, both
`stopped` and `stop-failed` count as "the operation finished." This
means a caller has to treat `jvm-stop` as a best-effort request, not a
promise, and should not assume `jvm-start` afterward will succeed.

## One JVM per Emacs process

This is a JNI-level constraint, not a choice this project made. A
process can create one `JavaVM` through `JNI_CreateJavaVM` in the
straightforward case this module relies on. Running two independent
embedded JVMs inside one Emacs process is not something emacs-jni
chose not to support — it is not something JNI supports.

## No per-jar unload

Every jar you load goes into one shared `URLClassLoader` — see
[Architecture](architecture.md#why-jar-loading-goes-through-reflection).
A `URLClassLoader` has no operation to selectively unload one jar
while leaving others loaded. Giving each jar its own classloader would
make unloading possible, at the cost of jars no longer transparently
seeing each other's classes, and of every reflective call needing to
pick the right classloader instead of always using the one shared one.
This project chose the simpler, single-classloader model, and accepted
losing per-jar unload as a consequence.

## A restart after a crash or stop does not fully recreate the JVM

The `jni` crate, at the version this project pins (0.22.4), caches the
`JavaVM` pointer internally in a process-global `OnceLock`, once
`JNI_CreateJavaVM` has succeeded once. A `jvm-start` after a crash or a
`jvm-stop` transparently reuses that same cached pointer rather than
performing a genuine reattachment or recreation. This was discovered
by reading `jni`'s implementation, not predicted from its public
API — see the comment above `create_jvm` in `src/jvm_thread.rs`. The
same root cause makes `cargo test --lib`'s default parallel test mode
flaky, when two test modules race to be the first to call
`JavaVM::new()`. Running with `--test-threads=1` avoids the race.

## Overload resolution only partly considers argument types

When two Java methods share a name and an argument count, `jvm-call`
picks the first one reflection returns that accepts the given
arguments, and falls through to the next same-arity candidate only if
marshalling the first one's parameters fails. This is not the same
algorithm the Java compiler uses to resolve overloads at compile time,
which considers assignability and specificity across the whole
candidate set at once. Implementing that fully, against a set of
methods only known at call time through reflection, is a
substantially bigger piece of work than this module currently does.
The gap that remains: `lisp_arg_to_jvalue`'s non-numeric argument
handling (strings, `nil`, `t`, wrapped objects) accepts any declared
target type without checking it, unlike the numeric arms, which do
check range. A wrong-typed non-numeric argument can pass marshalling
and only fail later, inside the actual method call, in a way the
fall-through does not retry on. See
[Interop design](interop-design.md#why-marshalling-is-type-directed-not-value-directed)
for why the numeric and non-numeric cases are not symmetric to begin
with.

## A panic inside a call's spawned thread vanishes silently

Each `jvm-call`/`jvm-new` runs on its own freshly spawned OS thread —
see [Architecture](architecture.md#a-call-gets-its-own-thread-not-the-worker-thread).
That thread is spawned with no `catch_unwind` and is never joined. A
panic inside it prints Rust's default panic message to stderr and
otherwise disappears — the corresponding `-sync` caller in Lisp simply
hangs until its timeout, with no more specific diagnostic. Unlike the
limitations above, this one is not a considered trade-off. It is a
known gap this project has not yet closed.

## A failed callback registration can leak

Passing a Lisp function into `jvm-call` registers it in a Lisp-side
table before the corresponding Java-side proxy object is confirmed to
exist — registration has to happen first, since the handle it produces
is what gets sent to Java as part of the call. If the call then fails
before that proxy is actually constructed (for example, because the
target class does not exist), the registration is never cleaned up,
since the cleanup path is the proxy's own eventual garbage collection,
and a proxy that was never constructed is never collected. Reordering
this — confirming success before registering — would need the Java
side to hand back a real proxy reference before the call it is an
argument to has even happened, which the current call shape does not
support. See
[How to pass a Lisp function to Java](../how-to-guides/pass-a-lisp-function-to-java.md#avoid-a-callback-table-leak)
for the practical impact.

## The GC-notification path has no automated test

Bidirectional GC notification (see
[Architecture](architecture.md#why-lisp-functions-and-jvm-objects-need-bidirectional-gc-notification))
depends on a real garbage-collection event happening on each side.
Triggering one deterministically, inside a short-lived `emacs --batch`
test process, is the obstacle — neither Emacs's nor the JVM's collector
promises to run on demand within a bounded window. This path has been
manually verified, but not automated. The same holds for the third
wrapper, a collected `LispObject` freeing its `GlobalRef`: it depends
on a JVM garbage-collection event, so it is documented and manually
checked, not asserted by a test.

## `jvm-call`/`jvm-new` trust the caller completely

Reflective dispatch on a Lisp-supplied class name and method name has
no access restriction: any Lisp caller can invoke any method on any
loaded class, including something like `Runtime.exec`. This is an
accepted assumption, not an oversight — Lisp code running inside Emacs
already has unrestricted access to Emacs and the local machine through
ordinary Elisp, so restricting what it can reach through the JVM would
not change the actual security boundary, only add friction to using
it.

## `*ns*` does not persist across separate Clojure eval calls

Because each `jvm-clojure-eval-string` call runs on its own freshly
spawned thread, and Clojure's current-namespace pointer is
thread-local, a namespace switch made by `(in-ns ...)` in one call is
invisible to the next call — that call runs on a different thread,
which sees Clojure's root namespace binding, not whatever the previous
thread set. Pinning every Clojure eval call to one fixed, dedicated
thread would fix this, at the cost of losing the concurrency and
isolation the per-call thread model buys everywhere else. This project
chose not to special-case Clojure eval calls that way. The nREPL
server does not share this limitation: once started, it manages its
own session and thread state internally, independent of the per-call
dispatch model that everything else in this module uses. See
[How to evaluate Clojure code](../how-to-guides/evaluate-clojure-code.md#switch-namespaces-inside-a-single-call)
for the workaround.

## A synchronous Clojure-to-Lisp call waits for Emacs to be free

Every `dev.ublt.emacs.clj` call — `intern`, `apply`, `type-of`, and so
on — runs on the Emacs main thread, because it operates on Lisp values
through an `Env` that is only valid there. The JVM thread that made the
call blocks until Emacs services it, which happens the next time Emacs
pumps the worker-to-Lisp pipe: while idle, or inside a `-sync`
wrapper's `accept-process-output` loop. See
[Architecture](architecture.md#why-clojure-calls-into-lisp-run-on-the-emacs-main-thread).

The consequence is that such a call cannot complete while Emacs is busy
in a long, uninterrupted Lisp computation — it waits until Emacs
returns to its event loop. This is inherent to Emacs's single-threaded
execution model, not a choice this module made; a per-call timeout
bounds the wait rather than letting a stalled call hang forever.

## Lisp values held by Clojure are opaque

Scalars, strings, and interned symbols/keywords from `apply` convert to
Clojure values, but any other Lisp value that Clojure holds through
`dev.ublt.emacs.clj` — a cons, vector, hash table, buffer, or an
uninterned symbol — is a `LispObject`, an opaque handle, not marshalled
Clojure data. You can inspect it (`type-of`, `is-not-nil?`, `eq`,
`prin1-to-string`) and pass it back into `apply`, but you cannot pull
its structure apart on the Clojure side. Marshalling those remaining
Lisp values into Clojure data was a non-goal, for the same reason the
reverse (rich Clojure-to-Lisp marshalling of vectors and maps) is:
neither type universe maps onto the other in a single obvious way. See
[Interop design](interop-design.md#why-a-lisp-value-becomes-an-opaque-java-object).

A `LispObject`'s underlying Lisp value stays alive only as long as the
`LispObject` is reachable on the JVM side; a cross-runtime reference
cycle through one would leak, the same as any other cross-runtime cycle
(see [Interop design](interop-design.md#the-no-cycles-assumption)).

## The nREPL server ships unhardened

`jvm-nrepl-start` uses nREPL's own defaults: loopback-only usage
assumed, no authentication, no TLS, no middleware selection. Anyone
who can reach the port can evaluate arbitrary code with the
permissions of the Emacs process. Hardening an nREPL server —
authentication, TLS, middleware choices — is an application-level
decision with several reasonable answers depending on deployment, and
this module deliberately did not pick one on the caller's behalf. It
hands back the same `nrepl.server/start-server` primitive a hardened
setup would build on, reachable directly through `jvm-call` if the
defaults are not enough. See
[How to run an nREPL server](../how-to-guides/run-an-nrepl-server.md#restrict-who-can-connect).
