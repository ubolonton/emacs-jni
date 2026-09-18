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

## Where a value converts, and why there is a middle form

A value crossing between Lisp and the JVM passes through two converters,
not one: a Lisp-side half that speaks a live `emacs::Value` and needs
the main-thread `Env`, and a JVM-side half that speaks a JNI
`jvalue`/`JObject`. Between them sits an owned, `Env`-free,
JNI-frame-free carrier — `commands::BridgeValue` — that both halves
understand. Four converters cover the base directions; a `jvm-call`'s
arguments and a callback's return reuse the Lisp-to-JVM pair, and a
`jvm-call`'s return and a callback's arguments reuse the JVM-to-Lisp
pair:

- Lisp to JVM: `lisp_value_to_arg` reads the Lisp value, then
  `lisp_arg_to_jvalue` builds the Java value.
- JVM to Lisp: `jvm_value_to_lisp_arg` reads the Java value, then
  `bridge_value_to_value` builds the Lisp value.

The Clojure layer adds a thin JVM-side wrapper on the same edge —
`->lisp`/`->clj` in `emacs-jni.clj` map a Clojure symbol or keyword to
and from a `LispSymbol` marker — and two direction-named enums,
`LispInput` (into Lisp) and `LispOutcome` (out of Lisp), that compose
`BridgeValue` rather than restate its scalar cases.

The middle form is not decoration. The async path runs the JVM-side
work on a different OS thread from Lisp, and a `JValueOwned` or
`emacs::Value` cannot cross a thread — each is bound to a JNI local
frame or to the main-thread `Env`. `BridgeValue` is the owned snapshot
that can sit in the command queue between the two threads; objects
reduce to a token there (see below), scalars inline.

The synchronous direct path (`jvm-call-direct`, and the Clojure calls
it drives) is the exception. It runs on the main thread, where both the
`Env` and an attached JNI env are live at once, so nothing crosses a
thread. There the middle `BridgeValue` is a code-reuse choice — the
same two converters serve both paths — not a requirement: a fused
Lisp-value-to-`jvalue` conversion would also work, trading the saved
intermediate for a second copy of the type-directed rules.

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
fact roughly what happens underneath (`elisp/jvm.el`'s
`jvm--lisp-callbacks` table, keyed by handle). The shim class exists to
make that handle behave like a normal, type-correct Java object from
the Java caller's point of view, so a method like
`Calc.callWithArg(LispCallable fn, long n)` can declare its parameter
type honestly, instead of taking `Object` and downcasting.

`LispNonLocalExit` exists for the same reason, in the other direction:
a Lisp `error` (or `throw`) signaled inside a callback needs to become
something Java code can catch as a real exception, not a string it has
to pattern-match. It is a plain `RuntimeException` carrying the
signal/throw's symbol-or-tag and data-or-value, marshaled to Java the
same way any other returned value is — a `LispSymbol` marker, a
`LispObject`, or a boxed scalar. Plain Java code catches it directly.
`emacs-jni.clj` rethrows it as a Clojure `ex-info`; see
[How to access Emacs Lisp from Clojure](../how-to-guides/access-lisp-from-clojure.md#handle-a-lisp-error).

## Why a Lisp value becomes an opaque Java object

The `emacs-jni.clj` namespace splits an `apply` result by type. A
scalar or string — number, string, `t`, `nil` — converts to the
matching Clojure value, the same way `jvm-call` marshals a Java scalar
into a Lisp scalar. An interned Lisp symbol or keyword converts to a
Clojure symbol or keyword. Every remaining value — a cons, a vector, a
hash table, a buffer, an uninterned symbol — becomes a Java
`LispObject`, an opaque handle.

Marshalling those remaining values into Clojure data was a non-goal.
Lisp's type universe (keymaps, buffers, markers, overlays, closures)
does not map onto Clojure data in any single obvious way, and most of
it is only meaningful back in Lisp anyway. So a `LispObject` stays
opaque: you inspect it with `type-of`, `is-not-nil?`, `eq`, and
`LispObject.prin1ToString`, and you pass it back into `apply` — you do
not pull it apart on the Clojure side. Arguments going the other way
follow the same split: a Clojure symbol or keyword interns to a Lisp
symbol/keyword, a plain scalar converts to the matching Lisp value, and
a `LispObject` passes through by reference — so `(apply '+ 1 2)` needs
no manual wrapping. Converting in both directions means a Lisp
predicate's `t`/`nil` lands as Clojure `true`/`nil`, and a returned
symbol as a real Clojure symbol, both usable directly.

Symbols convert only when interned. Emacs symbol identity (`eq`) is by
interning, so an interned symbol round-trips `eq`-stably — a Clojure
symbol interns to the one Lisp symbol of that name, and back again —
while an uninterned symbol (`make-symbol`/`gensym`) would lose its
identity if flattened to a name, so it stays an opaque `LispObject`
whose `GlobalRef` holds that exact symbol. Clojure symbols have no
interned form of their own (only Clojure keywords are cached), so there
is no matching case in the other direction.

Underneath, a `LispObject` owns an `emacs::GlobalRef` — the `emacs`
crate's mechanism for a Lisp value that outlives a single module call.
The Java object holds a `long`, the boxed pointer to that `GlobalRef`,
rather than the reference itself. This is not a threading shortcut: a
Java field can hold only a primitive, and a `GlobalRef` can be
dereferenced or freed only on the Emacs main thread with a live `Env`.
Handing the Rust reference to Java directly is therefore impossible —
Java has nothing to hold it in, and could not use it off the main
thread if it did. The `long` is a token Java parks and hands back to a
main-thread native, which is why every `LispObject` operation is
serviced on the Emacs main thread (see
[Architecture](architecture.md#why-clojure-calls-into-lisp-run-on-the-emacs-main-thread)).
This mirrors the other object direction — a JVM object returned to Lisp,
which Lisp likewise holds as an opaque `user-ptr` token for a reference
only the JVM side can use.

There is no Rust-side handle table for a `LispObject`: the pointer is
the reference's sole owner, so its lifetime ends when the one Java
object is collected. Correctness depends on exactly one `LispObject`
holding a given pointer. `LispObject` is a `final` class with a `final
ptr` and a package-private constructor, is neither `Cloneable` nor
`Serializable`, and is built only from Rust — each crossing mints a
fresh `GlobalRef`. So no two instances can share a pointer, and the
free, driven by one `Cleaner` per instance, runs exactly once. A copy
path or a reused pointer would double-free; those guardrails are
load-bearing.

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
