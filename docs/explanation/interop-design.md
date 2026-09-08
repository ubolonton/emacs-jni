# Interop design

This page discusses why `jvm-call`/`jvm-new` work the way they do. For
argument and return-value facts, see the
[Lisp API reference](../reference/lisp-api.md#argument-and-return-value-marshalling).
For steps, see
[How to call Java from Lisp](../how-to-guides/call-java-from-lisp.md).

## Why reflection, not generated bindings

A common way to bridge two languages is to generate typed bindings
ahead of time, from a schema, an IDL, or a scan of the target classes.
emacs-jni does not do this. `jvm-call`/`jvm-new` dispatch entirely
through Java reflection, at call time, on a class name and member name
supplied as plain strings.

This choice trades compile-time safety for reach. There is no build
step that needs to know your classpath ahead of time, no generated
file to keep in sync, and no limit on which classes are callable — any
class on the shared classloader is reachable the moment it is loaded.
The cost is that a typo in a class or method name, or a mismatched
argument type, only surfaces at call time, as a returned error rather
than a compile failure. For a module meant to reach into whatever jar
a user points it at — including Clojure and nREPL jars nobody
generated bindings for — reach mattered more than compile-time
checking.

## Why marshalling is type-directed, not value-directed

A Lisp integer can become a Java `int`, `long`, `float`, `double`, or a
boxed `BigInteger`, depending on what the target parameter declares —
see the reference's marshalling table. The alternative would be
picking one fixed Java type per Lisp type (for example, every Lisp
integer always becomes a `long`), and relying on Java's own numeric
widening or autoboxing to bridge the rest.

Type-directed marshalling was chosen because reflection already
exposes the target parameter's declared type before the call happens,
so consulting it costs nothing extra, and it lets a single Lisp
integer literal call `addLong(long, long)` and `addBig(BigInteger,
BigInteger)` without the caller doing anything different. The range
check on `int`/`short`/`byte` targets — a value too large signals a
marshalling error rather than silently truncating — follows the same
reasoning as the type-directed design generally: catch a mismatch
before it reaches Java as a wrong value, since a wrong value that
compiles and runs is far harder to notice than a call that returns an
error.

Not every argument type gets the same treatment. Strings, wrapped JVM
objects, and Lisp functions marshal to their one obvious Java shape
regardless of the target parameter's declared type — there is no
equivalent range check for them, since there is no equivalent question
of which numeric width to pick. This is also why
[overload resolution](limitations-and-tradeoffs.md#overload-resolution-only-partly-considers-argument-types)
can behave differently for numeric arguments than for string or object
arguments — the marshalling rules it depends on are not symmetric.

## Why a Lisp function becomes a real Java object

Passing a Lisp function into `jvm-call` wraps it as an instance of a
small Java shim class (`LispCallable`, backing `LispFunction`),
compiled once at build time and injected into the running JVM with
`DefineClass` at `jvm-start`. Calling it from Java returns a
`CompletableFuture`, which resolves once the Lisp side replies. Java
code holds and calls this object exactly like any other Java object of
that type — it does not need to know it is backed by Lisp.

The alternative — handing Java a bare integer handle and a
side-channel convention for "calling" it — was available, and is in
fact roughly what happens underneath (`elisp/emacs-jni.el`'s
`jvm--lisp-callbacks` table, keyed by handle). The shim class exists to
make that handle behave like a normal, type-correct Java object from
the Java caller's point of view, so a method like
`Calc.callWithArg(LispCallable fn, long n)` can declare its parameter
type honestly, instead of taking `Object` and downcasting.

`LispError` exists for the same reason, in the other direction: a Lisp
`error` signaled inside a callback needs to become something Java code
can catch as a real exception, not a string it has to pattern-match.

## Why a Lisp value becomes an opaque Java object

The `dev.ublt.emacs.clj` namespace splits an `apply` result by type. A
scalar or string — number, string, `t`, `nil` — converts to the
matching Clojure value, the same way `jvm-call` marshals a Java scalar
into a Lisp scalar. Every other Lisp value — a symbol (so `intern` and
`type-of` always), a cons, a vector, a hash table, a buffer — becomes a
Java `LispObject`, an opaque handle.

Marshalling those non-scalar Lisp values into Clojure data was a
non-goal. Lisp's type universe (symbols, keymaps, buffers, markers,
overlays, closures) does not map onto Clojure data in any single
obvious way, and most of it is only meaningful back in Lisp anyway. So
a `LispObject` stays opaque: you inspect it with `type-of`,
`is-not-nil?`, `eq`, and `LispObject.prin1ToString`, and you pass it
back into `apply` — you do not pull it apart on the Clojure side.
Arguments going the other way follow the same split: a `LispObject`
passes through by reference, while a plain Clojure scalar converts to
the matching Lisp value, so `(apply (intern "+") 1 2)` needs no manual
wrapping. Converting scalars in both directions also means a Lisp
predicate's `t`/`nil` result lands as Clojure `true`/`nil`, usable in
an `if` directly rather than through `is-not-nil?`.

Underneath, a `LispObject` owns an `emacs::GlobalRef` — the `emacs`
crate's mechanism for a Lisp value that outlives a single module call.
The Java object holds the pointer to it; there is no Rust-side handle
table, so the `GlobalRef`'s owner is unambiguously the one Java object,
and its lifetime ends when that object is collected. A `GlobalRef` can
only be created, read, or freed with a live `Env`, which is why every
`LispObject` operation is serviced on the Emacs main thread — see
[Architecture](architecture.md#why-clojure-calls-into-lisp-run-on-the-emacs-main-thread).

## The no-cycles assumption

The bidirectional GC notification described in
[Architecture](architecture.md#why-lisp-functions-and-jvm-objects-need-bidirectional-gc-notification)
assumes the cross-runtime object graph has no cycles: a Lisp closure
that captures a JVM object is fine, and a JVM object that holds a
`LispCallable` or a `LispObject` is fine, but a cycle spanning both
runtimes — where each side's collector can only see the other side as
"still referenced" — would keep both objects alive forever, since
neither garbage collector can trace through the other's heap.

This is a deliberate scope boundary, not an oversight. Detecting or
breaking cross-runtime cycles needs a distributed garbage collection
scheme, which is a substantially larger design problem than this
module set out to solve. In practice, most interop code calls into the
other runtime and gets a result back, without building a permanent
mutual reference — the kind of usage this module is built for.
