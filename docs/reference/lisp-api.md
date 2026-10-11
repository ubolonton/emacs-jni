# Lisp API reference

Library `jvm` (`elisp/jvm.el`) loads the dynamic module and defines the core API. Library
`jvm-clj` (`elisp/jvm-clj.el`) adds Clojure functions. Namespace `emacs-jni.clj`
(`clojure-src/emacs_jni/clj.clj`) gives Clojure code access to Lisp values.

## Variants

Most operations have variants. The suffix identifies the thread and the blocking behavior.

| Suffix | Runs on | Caller | Result | Failure | Timeout argument |
|---|---|---|---|---|---|
| `-async` | worker thread | does not block | request ID; the result goes to an optional callback | the callback gets `OK` = `nil` and an error string | none |
| `-await` | worker thread | blocks; waits with `accept-process-output` | the value | signals `jvm-error` | optional, last, default 30 s; signals `jvm-timeout` |
| `-direct` | calling thread (normally the main thread), synchronously | blocks Emacs for the whole call | the value | signals `jvm-error`, or the original Lisp signal/throw | none |

The bare name is a `defalias`:

| Bare name | Alias of |
|---|---|
| `jvm-start`, `jvm-pause`, `jvm-stop` | `-await` (no `-direct` form exists) |
| `jvm-load-jar`, `jvm-call`, `jvm-new` | `-direct` |
| `jvm-clj-start`, `jvm-clj-require`, `jvm-clj-eval-string`, `jvm-clj-nrepl-start`, `jvm-clj-nrepl-stop` | `-direct` |

On the direct path, the main thread attaches to the JVM once and stays attached.
`Thread.currentThread()` and Java `ThreadLocal` values persist across direct calls. Other Lisp
threads attach for one call only. `jvm-call-async` runs each call on a new OS thread, so
`ThreadLocal` values do not persist across `-async` or `-await` calls.

## Functions (`jvm.el`)

### Lifecycle

| Function | Arguments | Returns | Notes |
|---|---|---|---|
| `jvm-status` | `()` | Current status symbol. See [Status values](#status-values). | Reads Lisp state only. |
| `jvm-start-async` | `(&optional classpath options)` | Not specified. | Starts a JVM, or resumes a `paused` one. Allowed from `not-started`, `stopped`, `start-failed`, `stop-failed`, `crashed`, `paused`. `nil` arguments use `jvm-classpath` and `jvm-options`. On resume, both are ignored. |
| `jvm-start-await` | `(&optional classpath options timeout)` | Status after `starting` ends: `ready` or `start-failed`. | |
| `jvm-pause-async` | `()` | Not specified. | Detaches the worker thread. The JVM and its state stay. Allowed from `ready`. |
| `jvm-pause-await` | `(&optional timeout)` | Status after `pausing` ends. | |
| `jvm-stop-async` | `()` | Not specified. | Destroys the JVM. Allowed from `ready`, `paused`. A later `jvm-start` in the same process is not supported. See [Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md). |
| `jvm-stop-await` | `(&optional timeout)` | Status after `stopping` ends: `stopped` or `stop-failed`. | |

### Jar loading

| Function | Arguments | Returns | Notes |
|---|---|---|---|
| `jvm-load-jar-async` | `(path &optional callback)` | Request ID. | Adds the jar file or classpath directory at `path` to the shared classloader. Does not expand `~` or relative paths. Calls `(callback OK ERR)`. Requires `ready`. If `path` does not exist, the error is `jar/classpath entry not found: PATH`. |
| `jvm-load-jar-await` | `(path &optional timeout)` | `nil`. | |
| `jvm-load-jar-direct` | `(path)` | `nil`. | Requires `ready`. |

All variants use one shared classloader. A class from a jar loaded on one path is visible on the
other paths.

### Calls

| Function | Arguments | Returns | Notes |
|---|---|---|---|
| `jvm-call-async` | `(target member args &optional callback)` | Request ID. | Calls `(callback OK RESULT ERR)`. Requires `ready`. |
| `jvm-call-await` | `(target member args &optional timeout)` | Result value. | |
| `jvm-call-direct` | `(target member args)` | Result value. | Requires `ready`. |
| `jvm-new-async` | `(class-name args &optional callback)` | Request ID. | `jvm-call-async` with `member` = `"<init>"`. |
| `jvm-new-await` | `(class-name args &optional timeout)` | New instance. | `jvm-call-await` with `member` = `"<init>"`. |
| `jvm-new-direct` | `(class-name args)` | New instance. | `jvm-call-direct` with `member` = `"<init>"`. |
| `jvm-ping` | `()` | `t`. | Module check. Does not use the JVM. |

Arguments of `jvm-call-*`:

| Argument | Value |
|---|---|
| `target` | A binary class name string (for example `"java.lang.String"`) for a static call or a constructor, or a wrapped JVM object for an instance call. |
| `member` | A public method name, or `"<init>"` for a public constructor. |
| `args` | A list. Each element is marshalled by the [rules below](#argument-and-return-value-marshalling). |

Member resolution:

1. Collect the public methods (`getMethods()`) or constructors (`getConstructors()`) with the
   same name and the same number of parameters, in the order that Java returns them.
2. For each candidate, marshal `args` to its parameter types. If marshalling fails, try the next
   candidate.
3. Invoke the candidate. If `invoke` throws a bare `IllegalArgumentException`, try the next
   candidate.
4. The first candidate that succeeds gives the result. No "most specific" rule applies.

Example: `(jvm-call "java.lang.String" "valueOf" '(42))` returns `"42.0"`, because a
floating-point overload comes first. See
[Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md).

A Lisp signal or throw from a Lisp callback that unwinds out of `jvm-call-direct` is raised again
in Lisp as the original signal or throw.

## Functions (`jvm-clj.el`)

Load with `(require 'jvm-clj)`. These functions have `-await` and `-direct` variants only. Each
step calls `jvm-call-await` or `jvm-call-direct`. See
[How to evaluate Clojure code](../how-to-guides/evaluate-clojure-code.md).

| Function | Arguments | Returns | Notes |
|---|---|---|---|
| `jvm-clj-start-await` | `(&optional timeout)` | Result of Clojure `require`. | If the status is not `ready`, calls `jvm-start-await`. Loads each entry of `jvm-clj-classpath`, then this package's `clojure-src/`, with `jvm-load-jar-await`. Requires `emacs-jni.clj`. Signals `error` if the JVM does not become `ready`. |
| `jvm-clj-start-direct` | `(&optional timeout)` | Result of Clojure `require`. | Same as `jvm-clj-start-await`, but requires `emacs-jni.clj` on the direct path. JVM start and jar loading use `-await`, with `timeout`. |
| `jvm-clj-require-await` | `(namespace &optional timeout)` | Result of Clojure `require`. | `namespace` is a string, for example `"clojure.set"`. |
| `jvm-clj-require-direct` | `(namespace)` | Result of Clojure `require`. | |
| `jvm-clj-eval-string-await` | `(code &optional timeout)` | Value of the last form. | `code` is Clojure source. Evaluates every top-level form in order. Requires `emacs-jni.clj` (see `jvm-clj-start`). `*ns*` resets after each call. |
| `jvm-clj-eval-string-direct` | `(code)` | Value of the last form. | Same as `-await`. `*ns*` resets after each call. |
| `jvm-clj-nrepl-start-await` | `(&optional port bind timeout)` | `(ACTUAL-PORT . SERVER)`. | Calls `jvm-clj-start-await`, requires `nrepl.server`, calls `start-server`. `port` default: 0 (the OS selects a free port). `bind`: listen address; default: nREPL's loopback default. `SERVER` is a wrapped JVM object. See [How to run an nREPL server](../how-to-guides/run-an-nrepl-server.md). |
| `jvm-clj-nrepl-start-direct` | `(&optional port bind)` | `(ACTUAL-PORT . SERVER)`. | Calls `jvm-clj-start-direct`. The other steps use the direct path. |
| `jvm-clj-nrepl-stop-await` | `(server &optional timeout)` | `SERVER` (`nrepl.server/stop-server` returns its argument). | `server` is the `SERVER` from `jvm-clj-nrepl-start-*`. |
| `jvm-clj-nrepl-stop-direct` | `(server)` | `SERVER`. | |

Results of `jvm-clj-eval-string-*` follow the [Java → Lisp rules](#java--lisp). A Clojure keyword
or symbol result returns as a Lisp keyword or symbol. Clojure collections return as wrapped JVM
objects.

## Clojure API (`emacs-jni.clj`)

Use an alias: `(require '[emacs-jni.clj :as l])`. `intern`, `apply`, and `eq` have the same
names as `clojure.core` functions.

| Function | Arguments | Returns | Wait limit |
|---|---|---|---|
| `intern` | `[s]` | Clojure symbol. A keyword if `s` starts with `:`. | 30 s |
| `apply` | `[f & args]` | Result of the Lisp call, converted. | none |
| `apply-async` | `[f & args]` | `CompletableFuture` of the converted result. | does not block |
| `require-symbol` | `[sym]` | `IFn` that calls `sym`'s Lisp function definition at call time (late binding). | as `apply` |
| `refer-symbols` | `[& syms]` (macro) | Defines one var per symbol in the current namespace, with value `(require-symbol 'sym)`. | as `apply` |
| `require-function` | `[sym]` | `IFn` that holds the function object from `indirect-function` at import time (early binding). | as `apply` |
| `refer-functions` | `[& syms]` (macro) | Defines one var per symbol in the current namespace, with value `(require-function 'sym)`. | as `apply` |
| `type-of` | `[v]` | Lisp type as a Clojure symbol. | 30 s |
| `is-not-nil?` | `[v]` | `false` if `v` is Lisp `nil`, else `true`. | 30 s |
| `eq` | `[a b]` | `true` if Lisp `eq` is non-nil. | 30 s |
| `->clj` | `[x]` | Clojure symbol or keyword for a `LispSymbol`. Any other value unchanged. | |
| `eval-for-lisp` | `[code]` | Value of the last form of `code` (via `load-string`). A keyword or symbol becomes a `LispSymbol`. Used by `jvm-clj-eval-string-*`. | |

Thread behavior of Lisp access:

| Calling thread | Behavior |
|---|---|
| Main thread, inside `jvm-call-direct` | The request runs immediately on the main thread. |
| Any other JVM thread (worker, nREPL, user threads) | The request goes into a queue. Emacs runs it on the main thread when it reads process output, for example when idle or during
an `-await` wait. The JVM thread blocks until the reply arrives, or until the wait limit. |

At the 30 s wait limit, the call throws `java.util.concurrent.TimeoutException`.

`require-symbol` and `require-function` accept a Clojure symbol, keyword, or string. A
namespace-qualified symbol `ns/name` names the Lisp symbol `ns/name`. The returned `IFn`
implements `invoke` for 0 to 5 arguments and `applyTo`. For more arguments, use
`clojure.core/apply`. `refer-symbols` and `refer-functions` accept only names that are valid
Clojure symbols.

`LispObject` methods:

| Method | Returns | Wait limit |
|---|---|---|
| `prin1ToString()` | Lisp `prin1-to-string` text of the value. | 30 s |
| `toString()` | Same as `prin1ToString()`. | 30 s |

### Lisp non-local exits in Clojure

A Lisp `signal` or `throw` during `apply` throws a typed exception. `apply-async` completes its
future exceptionally with the same exception. `deref` and `.get` wrap it in
`java.util.concurrent.ExecutionException`. Use `(.getCause e)`.

| Lisp exit | Class | Fields |
|---|---|---|
| `signal` | `emacs_jni.core.LispSignal` | `symbol`, `data` |
| `throw` | `emacs_jni.core.LispThrow` | `tag`, `value` |

Both classes extend `emacs_jni.core.LispNonLocalExit` (a `RuntimeException`), with fields `kind`
(0 = signal, 1 = throw), `symbolOrTag`, and `dataOrValue`. Field values are not converted to
Clojure values. Each field holds a `LispSymbol`, a `LispObject`, or a boxed scalar. Use `l/->clj`
to get a Clojure symbol or keyword. See
[How to access Emacs Lisp from Clojure](../how-to-guides/access-lisp-from-clojure.md).

## Variables

| Variable | Library | Type | Default | Description |
|---|---|---|---|---|
| `jvm-classpath` | `jvm` | list of strings | `nil` | Classpath entries for a new JVM. Ignored on resume from `paused`. |
| `jvm-options` | `jvm` | list of strings | `("-Xrs")` | JVM options for a new JVM, for example `"-Xmx512m"`. Ignored on resume from `paused`. `"-Xrs"` stops the JVM from installing signal handlers. See [Architecture](../explanation/architecture.md). |
| `jvm-status-hook` | `jvm` | hook | `nil` | Run with one argument, the new status symbol, on every transition, including `starting`, `pausing`, `stopping`. |
| `jvm-module-directory` | `jvm` | string | directory of `jvm.el` | Directory of the dynamic module. Read once, at load time. Set it before `(require 'jvm)`. `dev-paths.el` sets it to `target/debug/` for a checkout. |
| `jvm-clj-classpath` | `jvm-clj` | list of strings | `nil` | Jars or directories that `jvm-clj-start` loads (usually the Clojure and nREPL jars). `jvm-clj-start` also loads `clojure-src/`. |

## Buffers

| Buffer | Contents |
|---|---|
| `*jvm-log*` | One `[status] STATUS: DETAIL` line for each `start-failed`, `stop-failed`, or `crashed` transition. One `[LEVEL] TEXT` line for each worker-thread log message. Log messages also go to `message`. `-async` call and jar-load failures go only to their callbacks. |

## Status values

| Status | Kind |
|---|---|
| `not-started` | initial |
| `starting`, `pausing`, `stopping` | transient |
| `ready` | running |
| `paused` | worker detached, JVM alive |
| `stopped` | JVM destroyed |
| `start-failed`, `stop-failed`, `crashed` | failure; detail in `*jvm-log*` |

`jvm-start-async`, `jvm-pause-async`, and `jvm-stop-async` set the transient status before the
worker thread starts the operation. If the operation is not allowed, the function restores the
previous status and signals `jvm-illegal-transition`. All variants of `jvm-load-jar`, `jvm-call`,
and `jvm-new` also signal `jvm-illegal-transition` if the status is not `ready`.

## Argument and return value marshalling

`jvm-call-*` and `jvm-new-*` marshal each argument by its Lisp type and the declared Java
parameter type. Results and Lisp-callback arguments are marshalled by their Java runtime type.

### Lisp → Java

The Lisp value is classified in the table order. The declared parameter type then selects the row.

| Lisp value | Declared parameter type | Java value |
|---|---|---|
| `nil` | `boolean`, `java.lang.Boolean` | `false` |
| `nil` | any other | `null` |
| keyword `:name` | any | `LispSymbol` with `name` = `"name"`, `keyword` = `true` |
| integer in the signed 64-bit range | `int`, `short`, `byte` | the value; marshalling fails if out of range |
| integer in the signed 64-bit range | `long`, `java.lang.Object` | `long` (`Long` for `Object`) |
| integer in the signed 64-bit range | `float`, `double` | the value, converted |
| integer in the signed 64-bit range | `java.math.BigInteger` | `BigInteger` |
| integer in the signed 64-bit range | any other | marshalling fails |
| integer outside the signed 64-bit range | `java.math.BigInteger`, `java.lang.Object` | `BigInteger` |
| integer outside the signed 64-bit range | any other | marshalling fails |
| float | `float` | the value, converted |
| float | `double`, `java.lang.Object` | `double` (`Double` for `Object`) |
| float | any other | marshalling fails |
| string | any | `String` |
| wrapped JVM object | any | the object |
| `t` | any | `true` |
| any other symbol (also a symbol with a function definition) | any | `LispSymbol` with `keyword` = `false` |
| non-symbol function (lambda, closure, byte-code, subr) | any | `LispFunction` |
| any other value (vector, hash table, cons, …) | any | signals `jvm-invalid-argument` before the call |

A failed marshalling skips the candidate (see [Member resolution](#calls)). Boxed parameter
types other than those listed (for example `java.lang.Long` or `java.lang.Double`) do not accept
numbers.

`LispSymbol` (`emacs_jni.core.LispSymbol`) has public fields `name` (no leading `:`) and
`keyword`. `LispFunction` implements `emacs_jni.core.LispCallable`, with method
`CompletableFuture<Object> call(Object... args)`. On the main thread inside `jvm-call-direct`, the
function runs immediately, and the returned future is complete. On any other JVM thread, the call
puts a request in the queue. The future completes after Emacs runs the function on the main
thread. The function's return value is
marshalled as for a `java.lang.Object` parameter. A Lisp signal in the function completes the
future exceptionally with `LispSignal`. See
[How to pass a Lisp function to Java](../how-to-guides/pass-a-lisp-function-to-java.md).

### Java → Lisp

Applies to `jvm-call-*` and `jvm-new-*` results, `LispFunction` call arguments, and
`jvm-clj-eval-string-*` results. Primitive results arrive boxed.

| Java value | Lisp value |
|---|---|
| `void` result, `null` | `nil` |
| `Boolean` | `t` or `nil` |
| `Byte`, `Short`, `Integer`, `Long` | integer |
| `BigInteger` | integer (bignum if outside the fixnum range) |
| any other `Number` (`Float`, `Double`, `BigDecimal`, …) | float, from `doubleValue()` |
| `String` | string |
| `LispSymbol` | interned symbol, or keyword if `keyword` is `true` |
| any other object, including `Character` | wrapped JVM object |

### Clojure → Lisp

Applies to `f` and `args` of `apply`, `apply-async`, `require-symbol` and `require-function`
callables, and the arguments of `type-of`, `is-not-nil?`, `eq`.

| Clojure value | Lisp value |
|---|---|
| keyword | keyword |
| symbol | interned symbol |
| `LispSymbol` | interned symbol or keyword |
| `LispObject` | the original Lisp value |
| any other value | as in [Java → Lisp](#java--lisp) |

### Lisp → Clojure

Applies to the results of `apply`, `apply-async`, `intern`, `type-of`, and the callables.

| Lisp value | Clojure value |
|---|---|
| `nil` | `nil` |
| `t` | `true` |
| string | `String` |
| float | `Double` |
| fixnum | `Long` |
| bignum | `BigInteger` |
| keyword | keyword |
| interned symbol | symbol |
| any other value (cons, vector, uninterned symbol, wrapped JVM object, …) | `LispObject` |

## Error conditions

All conditions have the parent `error`.

| Condition | Data | Signalled by | Cause |
|---|---|---|---|
| `jvm-timeout` | `(FUNCTION ARGS...)` | `-await` functions | `timeout` elapsed. The operation can still complete later. |
| `jvm-invalid-argument` | `(VALUE)` | `jvm-call-*`, `jvm-new-*`, all variants | An element of `args` has a Lisp type with no marshalling rule. `-async` forms signal before they send the request. |
| `jvm-illegal-transition` | `(STATUS-STRING)` or `(MESSAGE)` | `jvm-start-*`, `jvm-pause-*`, `jvm-stop-*`; `jvm-load-jar-*`, `jvm-call-*`, `jvm-new-*` when the status is not `ready` | The current status does not allow the operation, or no JVM or worker thread runs. |
| `jvm-error` | `(MESSAGE)` | `-await` and `-direct` forms of `jvm-load-jar`, `jvm-call`, `jvm-new`; all `jvm-clj-*` functions | Class, member, or jar not found; no candidate accepted the arguments; a Java or Clojure exception. `MESSAGE` includes the exception class, its message, and its cause chain. |

`-async` forms do not signal for these failures. The callback gets `OK` = `nil` and the message
string in `ERR`.

`jvm-clj-start-*` and `jvm-clj-nrepl-start-*` signal `error` if the JVM does not become
`ready`.

On the direct path, a Lisp `signal` or `throw` from a Lisp callback that unwinds out of the Java
call is raised again as the original signal or throw, not `jvm-error`. On the async path, it
becomes `jvm-error`.
