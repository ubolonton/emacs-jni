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
| `jvm-call-direct` | `(target member args)` | The `RESULT` value itself. | Like `jvm-call`, but runs the whole call synchronously on the Emacs main thread — no worker round trip — and returns the value directly. A genuine Java/Clojure failure signals `jvm-error`; a Lisp signal/throw raised in a same-thread callback is reconstructed and re-raised as its original `(signal …)`/`(throw …)`. Blocks Emacs's main loop for the call's duration; a same-thread Clojure→Lisp callback made during the call runs directly (no deadlock). Experimental — see [Synchronous Lisp → Clojure calls](../explanation/limitations-and-tradeoffs.md#jvm-call-direct-blocks-emacs-while-it-runs). |
| `jvm-new-direct` | `(class-name args)` | The new instance. | `jvm-call-direct` with `member` fixed to `"<init>"`. |
| `jvm-ping` | `()` | `t`. | A module sanity check. Does not touch the JVM. |

## Clojure Lisp-access library (`jvm-clj.el`)

A separate library, loaded with `(require 'jvm-clj)`. It lets Clojure
hold and manipulate Lisp values — see
[How to access Emacs Lisp from Clojure](../how-to-guides/access-lisp-from-clojure.md).

| Function | Arguments | Returns | Notes |
|---|---|---|---|
Each blocking function has a `-direct` counterpart that runs every JVM/Clojure
step on the Emacs main thread via `jvm-call-direct` — no worker round trip,
one always-attached thread (so thread-locals persist), and a reentrant
Clojure→Lisp callback cannot deadlock. A `-direct` function returns the value
directly and signals `jvm-error` on failure, rather than an `(OK RESULT ERR)`
tuple. Lifecycle (JVM start, jar loading) stays synchronous — it has no
main-thread form.

| `jvm-clj-start` | `(&optional timeout)` | `(OK RESULT ERR)` from the namespace `require`, on success. | Starts the JVM if needed, loads `jvm-clj-classpath` plus this package's `clojure-src` directory, and requires `emacs-jni.clj`. Signals an error if any step fails. |
| `jvm-clj-start-direct` | `(&optional timeout)` | The `require` `RESULT` value itself. | Like `jvm-clj-start`, but requires `emacs-jni.clj` through the direct (main-thread) path. JVM start and jar loading stay synchronous; a require failure signals `jvm-error`. |
| `jvm-clj-require` | `(namespace &optional timeout)` | `(OK RESULT ERR)`. | Calls Clojure's `require` on `NAMESPACE`, a string. |
| `jvm-clj-require-direct` | `(namespace)` | The `RESULT` value itself. | Direct counterpart of `jvm-clj-require`. |
| `jvm-clj-eval-string` | `(code &optional timeout)` | `(OK RESULT ERR)`. | Evaluates `CODE`, a string of Clojure source. Each call may run on a different worker thread, so a thread-local (e.g. `*ns*`) does not persist between calls — see [Known limitations](#known-limitations). |
| `jvm-clj-eval-string-direct` | `(code)` | The `RESULT` value itself. | Direct counterpart. Because the main thread stays attached across calls, a thread-local effect (e.g. `*ns*` after `in-ns`) *does* persist between successive direct evals. Blocks Emacs's main loop for the eval. |
| `jvm-clj-nrepl-start` | `(&optional port bind timeout)` | `(OK RESULT ERR)`, where `RESULT` is `(ACTUAL-PORT . SERVER)` on success. | Ensures `emacs-jni.clj` is loaded (via `jvm-clj-start`), then starts an nREPL server. `PORT` defaults to 0 (the OS picks a free port). `BIND`, if non-nil, is the interface address (e.g. `"0.0.0.0"`); defaults to nREPL's loopback — bind beyond loopback only when needed, see [How to run an nREPL server](../how-to-guides/run-an-nrepl-server.md#restrict-who-can-connect). `SERVER` is a wrapped JVM object, needed by `jvm-clj-nrepl-stop`. |
| `jvm-clj-nrepl-start-direct` | `(&optional port bind)` | `(ACTUAL-PORT . SERVER)`. | Direct counterpart: ensures `emacs-jni.clj` is loaded (via `jvm-clj-start-direct`) and starts the server, every step via `jvm-call-direct`. |
| `jvm-clj-nrepl-stop` | `(server &optional timeout)` | `(OK RESULT ERR)`. | `SERVER` is the value `jvm-clj-nrepl-start` returned. |
| `jvm-clj-nrepl-stop-direct` | `(server)` | The `RESULT` value itself. | Direct counterpart of `jvm-clj-nrepl-stop`. |

### Clojure API: `emacs-jni.clj`

Alias it — `(require '[emacs-jni.clj :as l])` — since `intern`,
`apply`, and `eq` shadow `clojure.core`. Each call blocks a JVM thread
until Emacs services it on its main thread.

| Function | Arguments | Returns |
|---|---|---|
| `intern` | `[s]` | a Clojure symbol, or a keyword when S starts with `:` |
| `apply` | `[f & args]` | the call result (see conversion below); blocks |
| `apply-async` | `[f & args]` | a `CompletableFuture` of the call result |
| `require-symbol` | `[sym]` | a callable value (`IFn`), late-bound: each call funcalls SYM's Lisp definition at that moment |
| `refer-symbols` | `[& syms]` (macro) | defs one var per SYM in the current namespace, each the `require-symbol` value |
| `require-function` | `[sym]` | a callable value (`IFn`), early-bound: resolves `indirect-function` once and holds that function object |
| `refer-functions` | `[& syms]` (macro) | defs one var per SYM in the current namespace, each the `require-function` value |
| `type-of` | `[v]` | a Clojure symbol (the Lisp type) |
| `is-not-nil?` | `[v]` | a boolean |
| `eq` | `[a b]` | a boolean |
| `lisp-signal?` | `[e]` | true if E is a `::lisp-signal` ex-info (the exception `apply` throws; for `apply-async`, unwrap the `ExecutionException` first — see below) |
| `lisp-throw?` | `[e]` | true if E is a `::lisp-throw` ex-info (the exception `apply` throws; for `apply-async`, unwrap the `ExecutionException` first — see below) |

`f` and each of `args` may be a Clojure symbol or keyword (→ a Lisp
symbol/keyword), a plain scalar (string, number, boolean, `nil` → the
matching Lisp value), a `LispObject` (passed by reference), or any other
JVM object (→ a wrapped `jvm-handle` value).

A result converts by type: a scalar or string (integer, float, string,
`t`, `nil`) → the matching Clojure value (`Long`, `Double`, `String`,
`true`, `nil`; a bignum → `BigInteger`); an interned Lisp symbol or
keyword → a Clojure symbol/keyword; anything else (a cons, vector, hash
table, or an *uninterned* symbol) → an opaque `LispObject`. So a Lisp
predicate's `t`/`nil` and a returned Lisp symbol are usable directly as
Clojure values. `LispObject.prin1ToString` returns the value's
`prin1-to-string` text as a Java `String` — the way to read an
otherwise opaque `LispObject`.

A Lisp non-local exit (`error`/`signal`, or `throw`) during `apply`
surfaces directly as a `clojure.lang.ExceptionInfo` (`ex-info`), not a
plain exception. `apply-async` completes its `CompletableFuture`
exceptionally with the same `ex-info`, but `deref`/`.get()` wraps it in
a `java.util.concurrent.ExecutionException` — read `(.getCause e)` for
the `ex-info`.

| Exit | `(ex-data e)` |
|---|---|
| `signal` | `{:type :emacs-jni.clj/lisp-signal :symbol <sym> :data <data>}` |
| `throw` | `{:type :emacs-jni.clj/lisp-throw :tag <tag> :value <value>}` |

`:symbol`/`:tag` and `:data`/`:value` are converted the same way as any
other result — a Lisp symbol or keyword to its Clojure counterpart,
anything else to an opaque `LispObject`. `lisp-signal?`/`lisp-throw?`
test which case an exception is. See
[How to access Emacs Lisp from Clojure](../how-to-guides/access-lisp-from-clojure.md#handle-a-lisp-error)
for a full example.

#### Import Lisp names as callables

`require-symbol`/`refer-symbols` and `require-function`/
`refer-functions` (in the table above) import Lisp names so they read
as ordinary Clojure calls:

```clojure
((l/require-symbol 'message) "hi")   ; late-bound: funcalls `message` each call
(l/refer-symbols message)
(message "hi")                       ; now calls Lisp's `message`
```

`require-symbol`/`refer-symbols` are late-bound: each call re-funcalls
the current Lisp definition of the symbol, so a later `defun`/`fset`
takes effect immediately. `require-function`/`refer-functions` are
early-bound: they resolve `indirect-function` once, at import time, and
keep calling that same function object, unaffected by a later
redefinition of the Lisp symbol.

`refer-symbols`/`refer-functions` only work for Lisp names that are
also legal Clojure `def` symbols (no name with a character illegal in a
Clojure symbol, such as embedded whitespace). For any other name, use
`require-symbol`/`require-function` with a string.

## Variables

| Variable | Kind | Default | Notes |
|---|---|---|---|
| `jvm-classpath` | `defvar`, list of strings | `nil` | Extra classpath entries. Read only on a genuinely fresh `jvm-start` — see the `jvm-start` row above. Set with `setq` before your first `jvm-start`. |
| `jvm-options` | `defvar`, list of strings | `("-Xrs")` | JVM flags (for example `"-Xmx512m"`). Same fresh-start-only rule as `jvm-classpath`. `"-Xrs"` stops the JVM from installing its own signal handlers — see [Architecture](../explanation/architecture.md). |
| `jvm-status-hook` | `defvar`, hook | `nil` | Run with one argument, the new status symbol, on every transition — including the transient `starting`/`pausing`/`stopping` states. |
| `jvm--module-path` | `defvar`, string | `$EMACS_JNI_MODULE_PATH`, or a path relative to `elisp/jvm.el` | Path to the built `.so` module, read once at `require` time. |
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
| `jvm-invalid-argument` | `jvm-call`/`jvm-new`, and their `-sync`/`-direct` forms. | An argument's Lisp value has no marshalling rule for any of the target member's parameter types. |
| `jvm-illegal-transition` | `jvm-start`, `jvm-pause`, `jvm-stop`, and their `-sync` forms; `jvm-call-direct`/`jvm-new-direct` with no running JVM. | The current `jvm-status` does not allow this transition. |
| `jvm-error` | `jvm-call-direct`/`jvm-new-direct`. | The synchronous call failed (missing class/method, a Java exception, or a Clojure exception — including an uncaught Lisp signal that unwound through Clojure). Unlike the async forms, which report failures in their return value, the direct forms signal. |

A Java exception, or a call to a nonexistent class or method, is
**not** signaled as a Lisp condition. It comes back as a descriptive
string in the `ERR` position of `(OK RESULT ERR)` (or `(OK . ERR)` for
`jvm-load-jar-sync`), with `OK` set to `nil`.

## Known limitations

- `jvm-clj-eval-string`: a namespace switch made by `(in-ns ...)`
  in one call does not carry over to the next call. See
  [Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md).
- `jvm-clj-require`/`jvm-clj-eval-string`/`jvm-clj-nrepl-start`/
  `jvm-clj-nrepl-stop` have no async form. Each one always blocks the
  calling thread.
