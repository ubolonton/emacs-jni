# Limitations and trade-offs

This page is the single list of known limitations. Each entry states what happens, why, and the
workaround. For signatures and defaults, see the [Lisp API reference](../reference/lisp-api.md).
For the design behind the call paths, see [Architecture](architecture.md).

## JVM lifecycle

### One JVM per Emacs process

An Emacs process can embed one JVM. All Lisp packages in that process share it, and they share
one classloader.

Why: JNI and the HotSpot JVM support one JVM per process.

Workaround: to get a second, independent JVM, run a second Emacs process.

### A stopped JVM cannot restart in the same process

`jvm-stop` destroys the JVM. Its status becomes `stopped` or `stop-failed`. Both values mean
that the operation finished. A later `jvm-start` gives a JVM that does not work. It typically
reports `ready`, and then the first `jvm-load-jar` or `jvm-call` fails with `JNI call failed`.

Why: the JVM cannot initialize again in a process that destroyed one. Also, see the
[next entry](#a-restart-after-a-crash-or-stop-does-not-fully-recreate-the-jvm).

Workaround: to pause the JVM and resume it later in the same process, use `jvm-pause`, not
`jvm-stop`. To reset the JVM, start a new Emacs process.

### A restart after a crash or stop does not fully recreate the JVM

The `jni` crate keeps the first JVM it creates for the life of the process. A later `jvm-start`
gets that same JVM again. It does not create a new one.

- After a crash (status `crashed`), the JVM is still alive. `jvm-start` starts a new worker
  thread on it. Classes, loaded jars, and Java objects from before the crash remain.
- After `jvm-stop`, the JVM is destroyed. `jvm-start` gets the destroyed JVM. See
  [the entry above](#a-stopped-jvm-cannot-restart-in-the-same-process).
- In both cases, the JVM ignores new values of `jvm-classpath` and `jvm-options`.

Why: this is how the `jni` crate stores the JVM. The module has no other way to reach the JVM.

Workaround: to change JVM options or get a clean JVM, start a new Emacs process.

### No per-jar unload

You cannot unload one jar. A loaded jar stays loaded until the process ends.

Why: all jars go into one shared `URLClassLoader`. `URLClassLoader` has no operation to remove
one jar. One shared loader lets every jar see the classes of the other jars.

Workaround: start a new Emacs process.

## Calls from Lisp to Java

### Overload resolution only partly considers argument types

`jvm-call` and `jvm-new` try each method or constructor with the correct name and argument
count. They try them in the order that `getMethods()` returns. They skip a candidate if an
argument does not marshal to its parameter type, or if `invoke` throws a bare
`IllegalArgumentException`. The first candidate that succeeds wins. Specificity has no effect.

The order of `getMethods()` is not specified. Thus a numeric argument can select an unexpected
overload:

```elisp
(jvm-call "java.lang.String" "valueOf" '(42))
;; => "42.0"  (a float or double overload won, not int)
```

A choice between a numeric and a non-numeric type is deterministic. For example, with a `long`
overload and a `String` overload, a Lisp integer cannot marshal to `String`, so `long` wins.

Why: the module finds methods through reflection at call time. It does not use the overload
algorithm of the Java compiler.

Workaround: call a method that has one overload for that argument count, or write a small Java
or Clojure wrapper. See [How to call Java from Lisp](../how-to-guides/call-java-from-lisp.md).

### Boxed-number parameters do not accept Lisp numbers

A Lisp integer does not marshal to a `java.lang.Long`, `Integer`, `Short`, or `Byte` parameter.
A Lisp float does not marshal to a `java.lang.Double` or `Float` parameter. The call skips that
candidate. If no other candidate fits, the call fails. Lisp numbers match only primitive types,
`Object`, and `BigInteger`.

Why: marshalling uses the declared parameter type. It has no rule for the boxed number types.

Workaround: call a method that takes a primitive or `Object` parameter. Or write a small Java or
Clojure wrapper that takes a primitive and calls the target method.

### A Java `char` result is a wrapped JVM object

A method that returns `char` or `Character` gives Lisp a wrapped JVM object, not an integer or
a string.

Why: `Method.invoke` boxes each primitive result. `Character` is not a `Number`, so no numeric
conversion applies to it.

Workaround: call `toString` on the result to get a one-character string:

```elisp
(let* ((builder (jvm-new "java.lang.StringBuilder" '("abc")))
       (ch (jvm-call builder "charAt" '(0))))   ; a wrapped JVM object
  (jvm-call ch "toString" nil))
;; => "a"
```

### `jvm-call` trusts the caller

`jvm-call` and `jvm-new` can call any public method on any class that the JVM can load. This
includes methods such as `Runtime.exec`. There is no access control.

Why: Lisp code in Emacs already has full access to Emacs and to the local machine. An access
control on the JVM side does not change that boundary.

Workaround: do not pass class or method names from untrusted input.

### A panic in an async call thread is lost

Each `-async` or `-await` call runs on a new OS thread. If the module code panics on that thread,
the panic message goes to stderr, and nothing reports it to Lisp. An `-await` caller signals
`jvm-timeout` when its TIMEOUT expires. An `-async` callback is never called.

Why: the module does not catch panics on that thread and does not wait for the thread to end.

Workaround: for `-async` calls, use your own timer if you must detect a lost result.

## Lisp functions passed to Java

### A failed call can leak a callback-table entry

When Lisp passes a Lisp function to Java, Lisp first adds the function to a table. The JVM
removes the entry later, when it collects the `LispFunction` for that entry. If the call fails
before the JVM makes the `LispFunction`, the entry stays in the table. Examples: the class does
not exist, no method has that name and argument count, or a later argument cannot marshal. Each
failed attempt adds one entry, also for the same function value.

Why: the table entry must exist before the call, because the JVM receives the entry's key as
part of the call.

Workaround: do not retry a failing call with a Lisp-function argument in a loop.

### A `LispFunction` lives only while Java holds it

A `LispFunction` works while the JVM can reach it. When the JVM collects it, Lisp removes the
function from its table. Each call that passes a Lisp function makes a new `LispFunction`, also
for the same function value. Thus two passes give two different Java objects.

When overload resolution tries more than one candidate, each candidate gets its own
`LispFunction` for the same table entry. When the JVM collects the `LispFunction` of a rejected
candidate, Lisp removes the shared entry. After that, a call through the `LispFunction` that the
chosen method keeps fails with `no such callback handle`.

Why: the JVM garbage collector controls the lifetime of each `LispFunction`. Its cleanup removes
the table entry and does not know about other `LispFunction` objects.

Workaround: if Java keeps a Lisp function after the call returns, pass it to a method that has
one overload for that argument count.

### A `LispFunction` result is not available inside a direct call

`LispFunction.call` always puts the request in the queue that Emacs drains on the main thread.
It does this also inside a `jvm-call-direct`. If Java then waits for the result on the main
thread, the wait cannot finish. It ends only when its own timeout expires. A wait with no
timeout blocks Emacs permanently.

Why: Emacs drains the queue only when the main thread is free. During a direct call, the main
thread runs the Java code.

Workaround: call a Java method that waits for a `LispFunction` result with `jvm-call-await` or
`jvm-call-async`.

## Clojure

### `*ns*` resets after every eval, and `ThreadLocal` values persist only on the direct path

A change to `*ns*` (for example from `in-ns`) does not reach the next `jvm-clj-eval-string` call.
This applies to the direct path and to `-await`. The same applies to a `set!` on
`*warn-on-reflection*`, `*unchecked-math*`, or `*data-readers*`.

Why: each eval calls `load-string`. `load-string` binds `*ns*` and other compiler vars for that
eval, and removes the bindings when it returns. A `binding` form is also scoped to its eval.

Java `ThreadLocal` values persist across direct calls on the main thread. The main thread
attaches to the JVM once and stays attached. `-await` and `-async` calls run on a new JVM thread
each time, so `ThreadLocal` values do not persist there. A direct call from another Lisp thread
attaches for that call only, so they do not persist there either.

Workaround: put the `in-ns` form and the code in the same code string. See
[How to evaluate Clojure code](../how-to-guides/evaluate-clojure-code.md).

### A synchronous Clojure-to-Lisp call waits for Emacs to be free

The functions of `emacs-jni.clj` (`apply`, `intern`, `type-of`, `eq`, `is-not-nil?`), the
callables from `require-symbol`, `require-function`, `refer-symbols`, and `refer-functions`, and
`LispObject.prin1ToString` use Lisp values. Lisp values are available only on the main thread.

- On the thread that runs a `jvm-call-direct`, the request runs immediately on that thread.
- On other JVM threads (async call threads, nREPL threads, Clojure futures), the request goes
  into a queue. Emacs runs it when it next drains the queue: when it is idle, or inside an
  `-await` wait loop. The JVM thread waits for the reply.

While Emacs runs a long Lisp computation or a direct call, the queued request waits:

| Operation | Maximum wait off the main thread |
|---|---|
| `intern`, `type-of`, `is-not-nil?`, `eq`, `prin1ToString` | 30 s, then `TimeoutException` |
| `apply`, and the `require-*`/`refer-*` callables | No timeout |

A `LispFunction.call` from Java always goes into the queue, also during a direct call. See
[A `LispFunction` result is not available inside a direct
call](#a-lispfunction-result-is-not-available-inside-a-direct-call).

Do not let a direct call wait for another JVM thread that calls Lisp. The main thread cannot
drain the queue during the direct call, so both threads wait.

Why: Emacs runs Lisp on one thread. Only the main thread can read and change Lisp values.

Workaround: use `apply-async` when the JVM thread must not block. Keep Lisp work on the main
thread short. See [How to access Emacs Lisp from Clojure](../how-to-guides/access-lisp-from-clojure.md).

### Lisp values held by Clojure are opaque

Integers, floats, strings, `nil`, `t`, and interned symbols and keywords convert to Clojure
values. Each other Lisp value is a `LispObject`. Examples are a cons, a vector, a hash table, a
buffer, and an uninterned symbol. Clojure cannot read the structure of a `LispObject`. It can
inspect it with `type-of`, `is-not-nil?`, `eq`, and `str` (which calls `prin1ToString`), and it
can pass it back to `apply`.

In the other direction, a Clojure vector or map returns to Lisp as a wrapped JVM object.

Why: the two type systems do not map onto each other in one clear way. See
[Interop design](interop-design.md).

Workaround: apply a Lisp function that returns a scalar or a string, for example `car`,
`length`, or `prin1-to-string`.

### `LispSignal` and `LispThrow` fields are not Clojure values

The `symbol` and `data` fields of a `LispSignal`, and the `tag` and `value` fields of a
`LispThrow`, are not converted to Clojure values. Each one holds a `LispSymbol`, a `LispObject`,
or a boxed scalar.

Why: Java code creates these exceptions, so their fields use the Java-side types.

Workaround: convert a field with `emacs-jni.clj/->clj`, for example `(l/->clj (.-symbol e))`.

### A `LispObject` keeps its Lisp value alive, and cycles leak

A `LispObject` keeps its Lisp value alive until the JVM collects the `LispObject`. Then Emacs
frees the value the next time it drains the queue. A reference cycle that crosses the two
runtimes is never collected. For example, a Lisp value holds a wrapped JVM object, and that
Java object holds a `LispObject` for the same Lisp value.

Why: each garbage collector sees only its own heap. Neither one can see a cycle through the
other.

Workaround: break the cycle by hand. For example, set the Java field to `null` when you no
longer need the value.

### The nREPL server has no authentication or TLS

`jvm-clj-nrepl-start` starts an nREPL server with no authentication and no TLS. By default it
listens on `127.0.0.1`. Each process that can connect to the port can run any code with the
permissions of the Emacs process. On loopback, this includes other local users.

Why: the correct security setup depends on the deployment. `jvm-clj-nrepl-start` does not choose
one for the caller.

Workaround: keep the default loopback address on a single-user machine. For TLS or other
options, call `nrepl.server/start-server` yourself through `jvm-call`. See
[How to run an nREPL server](../how-to-guides/run-an-nrepl-server.md).

## Blocking

### `jvm-call-direct` blocks Emacs while it runs

A direct call runs on the calling Lisp thread and holds the Emacs global lock until it returns.
All Lisp threads stop during that time. This is also true for a direct call from a `make-thread`
thread. Thus `make-thread` cannot run a blocking direct call in the background.

Why: the Emacs module API has no way to release the global lock.

Workaround: use `jvm-call-await` or `jvm-call-async` for slow calls. Or start the slow work on a
Clojure or Java thread and return immediately.

## Testing

### The garbage-collection paths have no automated test

Three cleanups depend on garbage collection. Emacs releases a wrapped JVM object. The JVM
releases a `LispFunction` table entry. The JVM frees the Lisp value of a `LispObject`. No
automated test covers these paths.

Why: neither garbage collector runs on demand within a known time in a short `emacs --batch`
process.
