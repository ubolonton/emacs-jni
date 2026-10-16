# Implementation

This page explains the Rust-level design for contributors. [Architecture](architecture.md)
describes the same system from the Lisp side. Read it first. This page names Rust modules, types,
and crate versions. Code comments hold the finer details.

## Module layout

The crate splits by the runtime that a piece of code needs, not by feature.

| Path | Contents |
|---|---|
| `src/protocol.rs` | Types that cross between the two halves: `BridgeValue`, `LispInput`, `LispOutcome`, `CallTarget`, `Status`. |
| `src/lisp/entry.rs` | Module `init`, the `jvm--*` defuns, and the module's error symbols. |
| `src/lisp/convert.rs` | Lisp values to and from `BridgeValue`, `LispInput`, and `LispOutcome`. |
| `src/lisp/inbox.rs` | The inbound queue (`Inbound`), the wakeup pipe writer, and the drain (`service_inbound`). |
| `src/lisp/runtime.rs` | The direct-path `Env` stack. |
| `src/jvm/entry.rs` | Shim class definition and the JNI natives of the shim classes. |
| `src/jvm/convert.rs` | JNI values to and from `BridgeValue`. `JvmObject` and `JvmHandle`. |
| `src/jvm/inbox.rs` | The worker thread: its `Cmd` channel, event loop, lifecycle, jar loading, and pending futures. |
| `src/jvm/invoke.rs` | Reflective calls and overload fall-through, for both paths. `call_direct`. |
| `src/jvm/runtime.rs` | Process-global JVM state: the `JavaVM`, the shared classloader, `STATUS`, main-thread attachment. |
| `src/jvm/test_support.rs` | `create_test_jvm`, for unit tests. |
| `java/emacs_jni/core/` | The shim classes: `Lisp`, `LispObject`, `LispSymbol`, `LispFunction`, `LispNonLocalExit`, and others. |
| `elisp/jvm.el`, `elisp/jvm-clj.el` | The public Lisp API, on top of the `jvm--*` defuns. |
| `clojure/emacs_jni/clj.clj` | The `emacs-jni.clj` namespace, on top of the `Lisp` shim class. |

In each half, `entry` holds the entry points that the other runtime calls. `inbox` holds the queue
into that half. `convert` holds the value conversion for that half. `runtime` holds its global
state.

Code in `lisp::` needs an `emacs::Env`, so it runs on the main thread. Code in `jvm::` needs a
JNI `Env`, so it runs on an attached thread. The direct path runs both on the main thread.

## Source lookup for module functions

`module-load` does not add the module's functions to `load-history`. `jvm.el` adds them, against
the module file. Then `symbol-file` returns the module file, and `find-function`, elisp xref
(`M-.`), and the `describe-function` link all call `find-function-search-for-symbol` with it.

`jvm.el` advises `find-function-search-for-symbol` to map the module file to the `#[defun]` in
`src/` (see `jvm--find-function-search-for-symbol`). The package archive holds `src/`, `java/`,
and `clojure/` with their checkout paths, so lookup also works for an installed package.

## Shim classes

`build.rs` compiles `java/emacs_jni/core/*.java` with `javac` into
`target/emacs-jni-shim-classes`. `jvm::entry` embeds the class files with `include_bytes!`.
`define_classes` defines them in the JVM right after creation, then registers their natives.

The classes go into the platform classloader. It is the parent of both the system classloader and
the shared jar loader, so both can see the classes. In the system classloader, jar classes such as
Clojure code could not see them.

`LispSymbol` exists for a similar reason. `clojure.lang.Symbol` and `Keyword` live in the jar
loader, and Rust cannot reach them by name. So Rust exchanges a `LispSymbol`, and `emacs-jni.clj`
converts it.

## Converters and the `BridgeValue` middle form

Every value crosses through `BridgeValue`. Four converters connect it to the two runtimes:

| Converter | Direction | Selects by |
|---|---|---|
| `lisp::convert::to_bridge` | Lisp value → `BridgeValue` | Lisp runtime type |
| `jvm::convert::from_bridge` | `BridgeValue` → JNI value | Declared Java parameter type |
| `jvm::convert::to_bridge` | JNI value → `BridgeValue` | Java runtime type |
| `lisp::convert::from_bridge` | `BridgeValue` → Lisp value | `BridgeValue` variant |

A `jvm-call` argument and a callback return value use the first two. A `jvm-call` result and a
callback argument use the last two.

`BridgeValue` exists because the worker path crosses threads. An `emacs::Value` is valid only with
its `Env` on the main thread. A `JValueOwned` is valid only in its JNI local frame. `BridgeValue`
is owned and `Send`, with scalars inline and objects as a `JvmObject`. So it can travel in a
`Cmd::Call` or an `Inbound::CallDone`.

On the direct path, nothing crosses a thread. There, `BridgeValue` is code reuse: one set of
type-directed rules serves both paths. A fused Lisp-to-JNI converter would save one intermediate
value, but it would duplicate those rules.

`BridgeValue::Callback` carries the key of a Lisp function in `jvm--lisp-callbacks`. It flows
only from Lisp to Java, where it becomes a `LispFunction`.

`jvm::convert::from_bridge` gives a `java.lang.Object` target a `Long`, `Double`, or
`BigInteger`. These match what `jvm::convert::to_bridge` produces, so a value that goes from Java
to Lisp and back keeps its type. `CompletableFuture#complete` takes an `Object`, so callback
replies depend on this.

Clojure-to-Lisp requests use two more types. `LispInput` is a value going into Lisp:
`Obj(ptr)` for a `LispObject`, `Symbol` for a `LispSymbol`, and `Scalar` for the rest.
`jvm::convert::to_input` builds it, and `lisp::convert::from_input` reads it. `LispOutcome` is a
result coming out of Lisp: a value, a boolean, a new `LispObject` pointer (`Ptr`), or a signal or
throw. `lisp::convert::to_outcome` builds it. Both types wrap `BridgeValue` for scalars. They do
not repeat its cases.

## JVM object ownership

A JVM object in Lisp is a `JvmHandle` `user-ptr`. It holds a `JvmObject`, which is an
`Arc<Global<JObject<'static>>>`. jni 0.22 gives `Global` no `Clone`, because a true clone needs a
JNI `Env`. The `Arc` lets a `BridgeValue` and a `CallTarget` share the reference without one.

Each Java-to-Lisp crossing creates a new `Global` and a new `JvmHandle`. Lisp never clones the
`JvmHandle`. Nothing keeps a handle table, so the `JvmHandle` is the owner.

Emacs can run the `JvmHandle` finalizer on a thread with no JNI attachment. So `Drop` sends an
`Arc` clone to the worker in `Cmd::ReleaseRef`. The worker drops it, and if that is the last
clone, `DeleteGlobalRef` runs on an attached thread. If the worker's clone drops first, the
finalizer thread does the delete. jni then attaches that thread and logs a warning. In both cases
the reference is deleted once.

## `LispObject` ownership

`lisp::inbox::box_value` makes an `emacs::GlobalRef` for a Lisp value and leaks it with
`Box::into_raw`. The pointer travels as `LispOutcome::Ptr(i64)`, and Java stores it in
`LispObject.ptr`. Requests that use the value carry the pointer as `LispInput::Obj`.
`lisp::inbox::borrow` binds it to the current `Env`, and does not free it.

The `LispObject` owns the `GlobalRef`. Its `Cleaner` runs `LispRefReleaseNotifier.run`, which
queues `Inbound::FreeRef`. The drain then frees the `GlobalRef` with `Box::from_raw` and
`GlobalRef::free`, on the main thread.

The design depends on one invariant: only one `LispObject` holds a given pointer. Two holders
would free it twice. So `LispObject` is `final`, its `ptr` is `final`, its constructor is
package-private, and it is neither `Cloneable` nor `Serializable`. Only Rust constructs it, and
each crossing boxes a new `GlobalRef`.

A pointer can also fail to reach Java. For example, a signal reply can convert one side and fail
on the other. `lisp::inbox::free_reply_ptr` frees such pointers, because no `Cleaner` will.

## Queue ordering

`lisp::inbox::submit` pushes an `Inbound` onto a `Mutex<VecDeque>` and writes one byte to the pipe.
`service_inbound` takes the whole queue and runs each message in FIFO order. Do not reorder it.

A `FreeRef` is sound only at its place in the queue. An earlier request can still borrow the same
pointer. That request was queued while its `LispObject` was reachable, because the native held a
reference to it. The `Cleaner` runs only after the object becomes unreachable. So its `FreeRef`
always comes later in the queue, and FIFO order frees the pointer after the last borrow. `Log`
and `Status` messages also depend on order.

One failing message must not stop the batch, because the drain already took the messages off the
queue. A Lisp-access request turns an error into a `Signal` or `Throw` reply. Other messages log
their errors.

The other direction uses an `mpsc` channel of `Cmd` to the worker. Replies to Java
(`Cmd::InvokeReply`, `Cmd::LispReply`) find their `CompletableFuture` by id in the pending table
of `jvm::inbox`.

## The worker thread

`jvm::inbox::run` creates the JVM, publishes it with `jvm::runtime::set_vm`, and loops over
`Cmd`. It wraps each command in `catch_unwind`. A panic sets the status to `crashed` and ends the
thread.

`Cmd::Call` starts a new thread that runs `jvm::invoke::handle_call`. That thread has no
`catch_unwind`, and nothing joins it. A panic in it loses the reply, so the `-await` caller times
out. Other commands are cheap and run on the worker thread itself.

## The direct path

`jvm--call-direct` calls `lisp::runtime::enter(env)` first. This pushes the raw `emacs_env`
pointer onto a thread-local stack, `ENV_STACK`. The returned `EnvScope` pops it on return and on
unwind. So the stack holds only pointers of module calls that are still running.

`jvm::invoke::call_direct` attaches the main thread permanently on the first call, and records
this in `jvm::runtime`. It then runs the call with `attach_current_thread_for_scope`. The main
thread reuses its permanent attachment. Another Lisp thread detaches on return, so it leaves no
attachment that could block `DestroyJavaVM`. `jvm--stop` detaches the main thread before it sends
`Cmd::Stop`, for the same reason.

A `Lisp` native checks `lisp::runtime::in_scope()`. If it is true, the native rebuilds an `Env`
from the top of the stack with `with_current_env`. It then runs `lisp::inbox::compute_reply`.
The queue drain runs the same request code, so both paths give the same results. emacs 0.21 roots
the values that an `Env` creates and frees them when the `Env` drops. So each native converts its
result to a `LispOutcome` before it returns.

`Lisp.applyAsync` and `LispFunction.call` return a `CompletableFuture`. They also check
`in_scope()`. If it is true, they compute the outcome on the current `Env` and return a future that
`complete_future_with_reply` already completed. `applyAsync` runs `compute_reply` on an
`Inbound::Apply`. `LispFunction.call` runs `lisp::inbox::invoke_callback`. It looks up the function
in `jvm--lisp-callbacks` and calls it. It turns a signal or throw into a `Signal` or `Throw`
outcome. If `in_scope()` is false, `LispFunction.call` queues `Inbound::Invoke`.

A Lisp signal or throw inside a direct call becomes a `LispNonLocalExit` in Java. If it reaches
the top of the call, `describe_caught_java_exception` stores it in `PENDING_NLE`. It does this
only while `ATTACH_DEPTH` is above zero. `call_direct` then returns `DirectError::NonLocalExit`,
and the defun raises the original signal or throw again.

`jvm::runtime` holds the state that both paths share. `get_or_create_loader` serializes creation
of the shared loader, so the two paths cannot create two loaders. The loader's parent is the
platform classloader. `jvm::invoke::resolve_and_invoke` sets the context classloader on every
call, when the loader exists.

## The jni 0.22.4 `JavaVM` singleton

jni 0.22.4 stores the first `JavaVM` in a process-global `OnceLock`. Each later `JavaVM::new`
returns that same VM. It ignores the new classpath and options. This has two consequences.

A restart after `jvm-stop` or a crash gets the destroyed VM back. `define_classes` is idempotent
for this reason, because a second definition of a class is a `LinkageError`. Only the first
`create_jvm` in the process treats a `define_classes` failure as fatal. A later failure is logged,
and the status becomes `ready`. JNI work on that VM then fails. See
[Limitations and trade-offs](limitations-and-tradeoffs.md).

The check for an existing VM has no lock. So parallel unit tests can race, and the loser gets
`AlreadyCreated`. `jvm::test_support::create_test_jvm` retries `JavaVM::new` up to 20 times, at
50 ms intervals. Retries find the winner's VM. Use it in every unit test that needs a JVM.

## Tests

`cargo test` runs the unit tests in `src/`. The build needs `javac` for the shim classes. jni
finds `libjvm` through `JAVA_HOME`. Unit tests cover the parts that need no Emacs, for example the
status state machine, queue order, and JNI conversion.

`.config/scripts/test.sh` runs the ERT integration tests in `tests/elisp/*.el`. It builds the
fixture jars and runs each test file in a separate `emacs -Q --batch` process. Each file needs a
separate process because the JVM and the module status are per-process singletons, and a restart
reuses the destroyed VM. Give file paths as arguments to run only those files.

The script has two modes, which load different code:

| Task | Loads | Also |
|---|---|---|
| `mise run test` | The checkout, through `dev-paths.el`: `elisp/`, `target/debug/`, and the sources at the root. | Builds the debug module. Without file arguments, runs the Rust unit tests first. |
| `mise run test:package` | The archive from `mise run package`, installed into `target/package-test-elpa/` and loaded through package.el (byte-compiled). | `tests/elisp/jvm-crash-test.el` skips itself: the release module has no `test-support`. |

The Clojure tests get their jars from `jvm-clj-ensure-jars`. The first run downloads them.

| Variable | Effect |
|---|---|
| `EMACS_JNI_TEST_FEATURES` | Cargo features for the build. `test-support` adds `jvm--test-panic`, which `tests/elisp/jvm-crash-test.el` needs. Without it, the test skips itself. |
