# Interop design

This page explains why values cross between Lisp and Java the way they do. For the marshalling
rules, see the
[Lisp API reference](../reference/lisp-api.md#argument-and-return-value-marshalling). For steps,
see [How to call Java from Lisp](../how-to-guides/call-java-from-lisp.md).

## Why reflection, not generated bindings

Many language bridges generate typed bindings ahead of time, from a schema or a scan of the
target classes. emacs-jni does not. `jvm-call` and `jvm-new` use Java reflection at call time.
The class name and member name are plain strings.

This trades compile-time checks for reach. No build step needs to know your classpath. No
generated file can go out of date. Any class becomes callable as soon as its jar is loaded. This
includes Clojure and nREPL, which have no bindings.

The cost is that errors appear only at call time. A wrong class name, method name, or argument
type signals `jvm-error`. A compiler does not catch it.

## Why marshalling is type-directed, not value-directed

A Lisp integer can become a Java `int`, `short`, `byte`, `long`, `float`, `double`, or
`BigInteger`. The declared type of the target parameter selects the Java type. An `Object`
parameter gets a `Long`. The other design gives
each Lisp type one fixed Java type, for example `long` for every integer. It then relies on Java
widening and boxing for the rest.

emacs-jni uses the declared type for two reasons:

- Reflection gives the declared parameter types before the call. So the lookup costs nothing
  extra.
- One Lisp integer can call both `addLong(long, long)` and `addBig(BigInteger, BigInteger)`. The
  caller writes the same code for both.

`int`, `short`, and `byte` targets get a range check. A value out of range fails marshalling. It
does not wrap to a wrong value. A wrong value that runs is harder to find than an error.

Only primitive types, `Object`, and `BigInteger` match a Lisp number. A boxed-number parameter,
such as `java.lang.Long`, `Integer`, or `Double`, does not accept a Lisp number.

Other Lisp types have one obvious Java form. Strings, wrapped JVM objects, symbols, and Lisp
functions marshal the same way for any parameter type. So numeric arguments and other arguments
behave differently in
[overload resolution](limitations-and-tradeoffs.md#overload-resolution-only-partly-considers-argument-types).

## Why a Lisp function becomes a Java object

A Lisp function passed to `jvm-call` becomes a `LispFunction`. This Java class implements the
`LispCallable` interface. Its `call` method returns a `CompletableFuture`, which completes when
the Lisp function returns.

Underneath, Lisp keeps the function in a table (`jvm--lisp-callbacks`), and Java gets an integer
key. A bare integer would work, but Java code would then need a custom calling convention. The
`LispFunction` class hides the key. So a Java method can declare a `LispCallable` parameter, for
example `callWithArg(LispCallable fn, long n)`. It does not need an `Object` parameter and a
cast.

Only a non-symbol function becomes a `LispFunction`: a lambda, closure, byte-code function, or
built-in function. A symbol becomes a `LispSymbol`, even when it has a function definition. A
symbol is data first. Clojure code opts in to calling a symbol with `require-symbol`.

`LispNonLocalExit` serves the other direction. A Lisp `signal` or `throw` in a callback must
reach Java as an exception that Java code can catch by type. It must not be a string that Java
code parses. `LispSignal` carries the signal symbol and data. `LispThrow` carries the catch tag
and value. Both extend `LispNonLocalExit`. Their fields hold a `LispSymbol`, a `LispObject`, or a
boxed scalar. They are not Clojure values. Use `emacs-jni.clj/->clj` to convert them. See
[How to access Emacs Lisp from Clojure](../how-to-guides/access-lisp-from-clojure.md#handle-a-lisp-error).

## Why a Lisp value becomes an opaque Java object

`emacs-jni.clj` sorts each Lisp value that it gets back by type:

- A number, string, `t`, or `nil` becomes the matching Clojure value. `t` becomes `true`.
- An interned symbol or keyword becomes a Clojure symbol or keyword.
- Any other value becomes a `LispObject`. Examples are a cons, vector, hash table, buffer, and
  uninterned symbol.

Rich marshalling of those other values was a non-goal. Lisp types such as keymaps, buffers,
markers, and closures have no single obvious Clojure form. Most of them are useful only in Lisp.
So a `LispObject` stays opaque. You inspect it with `type-of`, `is-not-nil?`, `eq`, and
`prin1ToString`, and you pass it back to `apply`. You do not take it apart in Clojure.

Arguments to `apply` follow the same rules in reverse. A Clojure symbol or keyword becomes a Lisp
symbol or keyword. A scalar becomes the matching Lisp value. A `LispObject` passes through as the
same Lisp value. So `(apply '+ 1 2)` needs no wrapping. A Lisp predicate's result works directly
in a Clojure conditional.

### Symbols convert only when interned

Lisp compares symbols with `eq`, and an interned symbol is the one symbol with its name. So an
interned symbol can travel as its name and keep its identity. An uninterned symbol, from
`make-symbol` or `gensym`, would lose its identity as a name. So it stays a `LispObject`, which
holds that exact symbol.

The reverse direction has no matching case. Clojure symbols are not interned, so every Clojure
symbol converts by name.

### Why a `LispObject` holds a number

A Lisp value that outlives one module call needs a global reference in Emacs. Emacs allows code to
read or free that reference only on the main thread. A JVM thread cannot use it.

So the `LispObject` does not hold the Lisp value. It holds a `long` that identifies the global
reference. Java only stores this number and gives it back to the module. The module turns it
back into the Lisp value on the main thread. This is why every `LispObject` operation runs on the
main thread. See
[Architecture](architecture.md#why-clojure-calls-into-lisp-run-on-the-emacs-main-thread).

Each `LispObject` owns its reference. When the JVM collects the `LispObject`, Emacs frees the
reference once. Two `LispObject`s must never share one number, because the second free would
corrupt memory. So `LispObject` is `final`, has no public constructor, and cannot be cloned or
serialized. Only the module creates one, with a new reference each time.

A wrapped JVM object is the mirror case. Lisp holds it, but only the JVM side can use the
reference inside. The worker thread deletes that reference after Emacs collects the wrapper.

## The no-cycles assumption

[Bidirectional GC notification](architecture.md#why-lisp-functions-and-jvm-objects-need-bidirectional-gc-notification)
assumes that no reference cycle spans both runtimes. A Lisp closure that holds a JVM object is
safe. A JVM object that holds a `LispCallable` or a `LispObject` is safe. A cycle through both
heaps is not safe. Each collector sees the other side as a live reference, and neither collector
can trace through the other heap. So neither object is ever freed.

Cycle detection across two runtimes needs a distributed garbage collector. That is outside the
scope of this module. Most interop code calls the other runtime and gets a result back. It does
not build permanent references in both directions.
