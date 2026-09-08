# Lisp API reference

Every function here is async by default: it returns immediately, and
the JVM worker thread does the work on a separate OS thread. Each async
function has a `-sync` counterpart with the same name plus a `-sync`
suffix. A `-sync` function blocks the calling Lisp thread until the
operation finishes, or until `timeout` seconds pass, whichever comes
first. `timeout` is always the last argument, is always optional, and
defaults to 30. Past the deadline, a `-sync` function signals
`jvm-timeout` instead of returning.

## Functions

| Function | Arguments | Returns | Notes |
|---|---|---|---|
| `jvm-status` | `()` | The current status symbol. | Synchronous. Reads local state. No round trip to the worker thread. |
| `jvm-start` | `(&optional classpath options)` | Not meaningful. | Starts a fresh JVM, or resumes one that is `paused`. Legal from `not-started`, `stopped`, `start-failed`, `stop-failed`, `crashed`, or `paused`. |
| `jvm-start-sync` | `(&optional classpath options timeout)` | The status once starting finishes: `ready` or `start-failed`. | |
| `jvm-pause` | `()` | Not meaningful. | Detaches the worker thread. The JVM and its state stay alive. Legal only from `ready`. |
| `jvm-pause-sync` | `(&optional timeout)` | The status once pausing finishes: `paused`. | |
| `jvm-stop` | `()` | Not meaningful. | Attempts a full JVM teardown. Legal from `ready` or `paused`. Best-effort — see [Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md). |
| `jvm-stop-sync` | `(&optional timeout)` | The status once stopping finishes: `stopped` or `stop-failed`. | |
| `jvm-load-jar` | `(path &optional callback)` | A request ID. | Adds `path` to the shared classloader. `callback`, if given, is called as `(callback OK ERR)`. Legal only when `ready`. Does not change `jvm-status`. |
| `jvm-load-jar-sync` | `(path &optional timeout)` | `(OK . ERR)`. | |
| `jvm-call` | `(target member args &optional callback)` | A request ID. | Reflectively invokes `TARGET.MEMBER(ARGS...)`. `TARGET` is a class-name string for a static call, or a wrapped JVM object for an instance call. `callback`, if given, is called as `(callback OK RESULT ERR)`. |
| `jvm-call-sync` | `(target member args &optional timeout)` | `(OK RESULT ERR)`. | |
| `jvm-new` | `(class-name args &optional callback)` | A request ID. | `jvm-call` with `member` fixed to the constructor's internal name, `"<init>"`. |
| `jvm-new-sync` | `(class-name args &optional timeout)` | `(OK RESULT ERR)`. | |
| `jvm-clojure-require` | `(namespace &optional timeout)` | `(OK RESULT ERR)`. | Calls Clojure's `require` on `NAMESPACE`, a string. Synchronous only — see [Known limitations](#known-limitations) below. |
| `jvm-clojure-eval-string` | `(code &optional timeout)` | `(OK RESULT ERR)`. | Evaluates `CODE`, a string of Clojure source. Synchronous only. See [Known limitations](#known-limitations) below. |
| `jvm-nrepl-start` | `(&optional port bind timeout)` | `(OK RESULT ERR)`, where `RESULT` is `(ACTUAL-PORT . SERVER)` on success. | `PORT` defaults to 0 (the OS picks a free port). `BIND`, if non-nil, is the interface address to listen on (e.g. `"0.0.0.0"`); it defaults to nREPL's loopback. Bind beyond loopback only when needed — see [How to run an nREPL server](../how-to-guides/run-an-nrepl-server.md#bind-to-a-routable-interface). `ACTUAL-PORT` is an integer. `SERVER` is a wrapped JVM object, needed by `jvm-nrepl-stop`. Synchronous only. |
| `jvm-nrepl-stop` | `(server &optional timeout)` | `(OK RESULT ERR)`. | `SERVER` is the value `jvm-nrepl-start` returned. Synchronous only. |
| `jvm-ping` | `()` | `t`. | A module sanity check. Does not touch the JVM. |

## Clojure Lisp-access library (`jvm-clj.el`)

A separate library, loaded with `(require 'jvm-clj)`. It lets Clojure
hold and manipulate Lisp values — see
[How to access Emacs Lisp from Clojure](../how-to-guides/access-lisp-from-clojure.md).

| Function | Arguments | Returns | Notes |
|---|---|---|---|
| `jvm-clj-start` | `(&optional timeout)` | `(OK RESULT ERR)` from the namespace `require`, on success. | Starts the JVM if needed, loads `jvm-clj-classpath` plus this package's `clojure-src` directory, and requires `dev.ublt.emacs.clj`. Signals an error if any step fails. |
| `jvm-clj-nrepl-start` | `(&optional port bind timeout)` | Same as `jvm-nrepl-start`. | `jvm-nrepl-start` (including its `bind` argument) after ensuring `dev.ublt.emacs.clj` is loaded. |
| `jvm-clj-nrepl-stop` | `(server &optional timeout)` | Same as `jvm-nrepl-stop`. | Passes through to `jvm-nrepl-stop`. |

### Clojure API: `dev.ublt.emacs.clj`

Alias it — `(require '[dev.ublt.emacs.clj :as l])` — since `intern`,
`apply`, and `eq` shadow `clojure.core`. Each call blocks a JVM thread
until Emacs services it on its main thread.

| Function | Arguments | Returns |
|---|---|---|
| `intern` | `[s]` | a `LispObject` (the interned symbol) |
| `apply` | `[f & args]` | a `LispObject` (the call result); blocks |
| `apply-async` | `[f & args]` | a `CompletableFuture<LispObject>` |
| `type-of` | `[v]` | a `LispObject` (the type symbol) |
| `is-not-nil?` | `[v]` | a boolean |
| `eq` | `[a b]` | a boolean |

`f` is a `LispObject` (from `intern`, or any Lisp function value).
`args` may be `LispObject`s or plain Clojure scalars (strings, numbers,
booleans, `nil`), which convert to the matching Lisp value. A Lisp
`error` completes the future exceptionally with `emacs_jni.LispError`.
`LispObject.prin1ToString` returns the value's `prin1-to-string` text
as a Java `String` — the only way to read an otherwise opaque
`LispObject`.

## Variables

| Variable | Kind | Default | Notes |
|---|---|---|---|
| `jvm-classpath` | `defvar`, list of strings | `nil` | Extra classpath entries. Read only on a genuinely fresh `jvm-start` — see the `jvm-start` row above. Set with `setq` before your first `jvm-start`. |
| `jvm-options` | `defvar`, list of strings | `("-Xrs")` | JVM flags (for example `"-Xmx512m"`). Same fresh-start-only rule as `jvm-classpath`. `"-Xrs"` stops the JVM from installing its own signal handlers — see [Architecture](../explanation/architecture.md). |
| `jvm-status-hook` | `defvar`, hook | `nil` | Run with one argument, the new status symbol, on every transition — including the transient `starting`/`pausing`/`stopping` states. |
| `emacs-jni--module-path` | `defvar`, string | `$EMACS_JNI_MODULE_PATH`, or a path relative to `elisp/emacs-jni.el` | Path to the built `.so` module, read once at `require` time. |
| `jvm-clj-classpath` | `defvar`, list of strings | `nil` | From `jvm-clj.el`. Extra classpath entries for `jvm-clj-start` (typically your Clojure/nREPL jars), added on top of `jvm-classpath`. `jvm-clj-start` also adds this package's `clojure-src` directory automatically. |

## Buffers

| Buffer | Contents |
|---|---|
| `*jvm-log*` | One line per `start-failed`/`stop-failed`/`crashed` status message, and per worker-thread `log` message. Does not receive `jvm-load-jar`/`jvm-call` failures — those come back in the function's own return value. |

## Status values

`not-started`, `starting`, `ready`, `pausing`, `paused`, `stopping`,
`stopped`, `start-failed`, `stop-failed`, `crashed`.

`starting`, `pausing`, and `stopping` are transient: the corresponding
async function's Lisp wrapper sets the status to one of these the
moment it is called, before the worker thread has done anything. If
the underlying primitive then signals an error, the wrapper reverts
`jvm-status` to whatever it was immediately before the call.

## Argument and return value marshalling

`jvm-call`/`jvm-new` marshal each Lisp argument to a Java value, based
on the target parameter's declared type. `jvm-call-sync`/
`jvm-new-sync` marshal the Java return value back to a Lisp value the
same way.

Lisp value passed as an argument:

| Lisp value | Target Java type | Marshals to |
|---|---|---|
| `nil` | `boolean` or `java.lang.Boolean` | `false` |
| `nil` | anything else | Java `null` |
| `t` | any | `true` |
| an integer | `int`, `short`, or `byte` | the same value, if it fits that type's range — otherwise a marshalling error |
| an integer | `long`, or a generic `java.lang.Object` parameter | a boxed `Long` |
| an integer | `float` or `double` | the same value, converted |
| an integer | `java.math.BigInteger` | a boxed `BigInteger` |
| a float | `float` or `double` | the same value |
| a float | a generic `java.lang.Object` parameter | a boxed `Double` |
| a bignum (an integer outside Emacs's fixnum range) | `java.math.BigInteger`, or a generic `java.lang.Object` parameter | a boxed `BigInteger` |
| a string | any | a Java `String` |
| a wrapped JVM object (returned earlier by `jvm-call`/`jvm-new`) | any | that object, at its real Java type |
| a Lisp function (`lambda` or a symbol naming one) | any | a real, callable Java object — see [Interop design](../explanation/interop-design.md) |

Java value returned, or passed as an argument into a Lisp callback:

| Java value | Lisp value |
|---|---|
| `void` | `nil` |
| `boolean`/`Boolean` | `t` or `nil` |
| `byte`/`short`/`int`/`long`, boxed or not | an integer |
| `char` | an integer (the character's code point) |
| `float`/`double`, boxed or not | a float |
| `String` | a string |
| `BigInteger` | a bignum |
| Java `null` | `nil` |
| anything else | a wrapped JVM object |

## Error conditions

| Condition | Signaled by | Meaning |
|---|---|---|
| `jvm-timeout` | Any `-sync` function, once its `timeout` elapses before the operation finishes. | The operation may still finish later. The caller stopped waiting for it. |
| `jvm-invalid-argument` | `jvm-call`/`jvm-new`, and their `-sync` forms. | An argument's Lisp value has no marshalling rule for any of the target member's parameter types. |
| `jvm-illegal-transition` | `jvm-start`, `jvm-pause`, `jvm-stop`, and their `-sync` forms. | The current `jvm-status` does not allow this transition. |

A Java exception, or a call to a nonexistent class or method, is
**not** signaled as a Lisp condition. It comes back as a descriptive
string in the `ERR` position of `(OK RESULT ERR)` (or `(OK . ERR)` for
`jvm-load-jar-sync`), with `OK` set to `nil`.

## Known limitations

- `jvm-clojure-eval-string`: a namespace switch made by `(in-ns ...)`
  in one call does not carry over to the next call. See
  [Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md).
- `jvm-clojure-require`/`jvm-clojure-eval-string`/`jvm-nrepl-start`/
  `jvm-nrepl-stop` have no async form. Each one always blocks the
  calling thread.
