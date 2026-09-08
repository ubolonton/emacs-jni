# Built-in Lisp access from Clojure — design

Date: 2026-09-08
Status: approved for planning

## Goal

Let Clojure code hold and manipulate Emacs Lisp values directly, as
opaque objects. This is the inverse of the existing `jvm-call` path
(Lisp reaches into the JVM). A new Clojure namespace
`dev.ublt.emacs.clj` exposes `intern`, `apply`, `apply-async`,
`type-of`, `is-not-nil?`, and `eq`. A new Elisp library `jvm-clj.el`
starts the Clojure side and wraps the existing nREPL controls.

Non-goal: marshalling arbitrary Lisp values to Clojure data. Lisp
values stay opaque on the JVM side; you inspect them via
`prin1-to-string`, `type-of`, `eq`, and `is-not-nil?`.

## Core constraint

Every operation is an `emacs::Env` method
(`intern`, `type_of`, `is_not_nil`, `eq`, `call` for apply and
`prin1-to-string`). `emacs::GlobalRef` needs an `&Env` to create,
bind, and free (`free_global_ref`). The `Env` exists only on the
Emacs main thread, during a module call.

Consequences:

- All Lisp work runs on the Emacs main thread.
- Clojure/JVM threads only carry opaque pointers and round-trip
  requests.
- Synchronous ops block the calling JVM thread until the main thread
  next pumps the pipe process (`accept-process-output` or idle) — the
  same pump that already services `invoke`. This is reentrant: a
  `clj/apply` issued from inside a `jvm-clojure-eval-string` is
  serviced by the outer `-sync` wrapper's own `accept-process-output`
  loop. A JVM-side timeout guards the case where Emacs is stuck in a
  long pure-Lisp computation.

## Object model: `LispObject` owns its `GlobalRef`

`GlobalRef` is `Send`+`Sync`; the JVM side owns it but never
dereferences it off the main thread. Standard JNI native-peer pattern:

- `emacs_jni.LispObject` holds `final long ptr`, where
  `ptr == Box::into_raw(Box::new(gref)) as jlong`. No Rust-side handle
  table, no counter, no lookup.
- **Create** (main thread, has `Env`): a result `Value` →
  `make_global_ref` → `Box::into_raw` → `ptr`. The `ptr` travels to
  the worker thread, which constructs `new LispObject(ptr)`.
- **Use** in an op: the JVM thread reads its argument `LispObject`s'
  `ptr` fields and puts them in the request. The main thread borrows
  `&*(ptr as *const GlobalRef)` and binds it with `Env`. Borrow only —
  never freed here.
- **Free** on GC: a `java.lang.ref.Cleaner` action (mirroring the
  existing `ReleaseNotifier`) enqueues `FreeRef(ptr)`; the main thread
  runs `Box::from_raw(ptr).free(env)`, exactly once.

Safety: an op is a method on, or takes as an argument, a *live*
`LispObject`, so that object's Cleaner cannot run mid-call. Only the
Cleaner path takes ownership of the box; op paths borrow.

## Request/reply flow

Requests originate on JVM threads but must run on the Emacs main
thread. The only Rust→main-thread channel is the pipe process. The
pipe carries only a wakeup; the payload rides a Rust queue so no Lisp
value is ever sexp-serialized.

1. A native method (JVM thread):
   - allocates `id = pending::next_id()`,
   - builds a `CompletableFuture`, `pending::register(id, future)`,
   - enqueues a `LispRequest` (carrying `id`, op, arg ptrs/scalars) on
     the Rust queue,
   - `channel::send("(lisp-request)\n")`,
   - sync variant: `future.get()` (blocks the JVM thread, with
     timeout); async variant: returns the future.
2. Emacs main thread, on `(lisp-request)`, calls the module primitive
   `jvm--service-lisp-requests` (gets `&Env`), which drains the queue.
   Per request it runs the Env op, wraps any result `Value` as a new
   `GlobalRef` box → `ptr`, and sends `Cmd::LispReply { id, value }`
   to the worker.
3. The worker (JVM-attached) builds the reply value and completes the
   future: a `LispObject(ptr)`, a boxed `Boolean`, a `String`, or an
   exceptional completion with `LispError`. Completion must route
   through the worker because only it is attached to the JVM.

`FreeRef` requests carry no `id` and produce no reply.

### Ordering invariant (load-bearing)

The queue is strict FIFO and drained in order on the main thread. This
is what keeps `apply-async` sound when the caller drops its argument
`LispObject`s immediately after the call: an argument's `FreeRef` can
only be enqueued after that object is unreachable, which is after
`applyAsync` returned — i.e. after the `Apply` request that read its
`ptr` was already enqueued. FIFO drain therefore always processes the
`Apply` before the `FreeRef`, so the borrowed `ptr` is still valid when
the op runs. Any change that reorders draining (priority lanes,
processing `FreeRef` eagerly) breaks this and must be rejected.

## Operation semantics

Executed inside `jvm--service-lisp-requests` on the main thread:

- `intern(name: String)` → `env.intern(name)` → `LispObject` (symbol).
- `apply-async(f, args)` → bind `f`'s `GlobalRef`, convert each arg
  (below), `env.call(f_value, args)` → result `Value` → `LispObject`.
  The native method returns the `CompletableFuture<LispObject>`
  immediately (derefable in Clojure with `@`). A Lisp `error` becomes
  an exceptional future completion with `LispError`, which surfaces on
  deref.
- `apply(f, args)` → the Clojure-level blocking form, `@(apply-async
  f args)`. No dedicated native.
- `type-of(v)` → `env.type_of(value)` → `LispObject` (type symbol).
- `is-not-nil?(v)` → `value.is_not_nil()` → `boolean`.
- `eq(a, b)` → `env.call("eq", [a, b])?.is_not_nil()` → `boolean`.
- `LispObject.prin1ToString()` → `env.call("prin1-to-string",
  [value])?` → `String`.

### Argument conversion

`apply`/`apply-async` take `Object[]`. Per element, on the JVM thread,
into the request:

- a `LispObject` → carried by `ptr`, bound to its `Value` on the main
  thread;
- a scalar (`String`, boxed `Long`/`Integer`/`Double`/`Boolean`,
  `null`) → converted to a Lisp `Value` on the main thread via the
  `emacs` crate's `IntoLisp`;
- any other JVM object → wrapped as a `(jvm-handle N)` via the
  existing `marshal::jvm_value_to_lisp_arg`, so it reaches Lisp as the
  existing JVM-object `user-ptr`.

`intern` takes a bare `String`.

## Components

### Rust

- New request queue module: `Mutex<VecDeque<LispRequest>>` plus
  `enqueue`/`drain`. `LispRequest` enum: `Intern`, `Apply`, `TypeOf`,
  `IsNotNil`, `Eq`, `Prin1`, `FreeRef`.
- Reuse `pending.rs` (CompletableFuture-by-id) and the worker command
  channel; add `Cmd::LispReply { id, value: LispReplyValue }` with
  `LispReplyValue` = `Ptr(i64) | Bool(bool) | Str(String) |
  Error(String)`. The worker builds the Java value and
  completes/exceptionally-completes the future (mirrors
  `handle_invoke_reply`).
- New `#[defun] jvm--service-lisp-requests(env)`: drains the queue,
  runs each op with `env`, boxes result `GlobalRef`s, emits
  `Cmd::LispReply`, and services `FreeRef` by
  `Box::from_raw(ptr).free(env)`.
- Native method registration for the new Java classes, alongside the
  existing four in `shim.rs`.

No `lisp_handles.rs` table: the `GlobalRef` is owned by the Java
`LispObject`'s `ptr`.

### Java (compiled at build, injected via `shim::define_classes`)

- `emacs_jni.LispObject`: `final long ptr`; `native String
  prin1ToString()`; `toString()` delegating to `prin1ToString()`;
  Cleaner-registered so GC enqueues `FreeRef(ptr)` (mirrors
  `ReleaseNotifier`; add a small `LispRefReleaseNotifier` or reuse the
  established pattern).
- `emacs_jni.Lisp`: static natives `intern(String)`,
  `typeOf(LispObject)`, `isNotNil(LispObject)`,
  `eq(LispObject, LispObject)`, `applyAsync(LispObject, Object[])`.
  The sync natives (`intern`, `typeOf`, `isNotNil`, `eq`, and
  `LispObject.prin1ToString`) block internally on their own reply
  future. There is no separate sync `apply` native — the Clojure
  `apply` derefs `applyAsync`.

### Clojure — `clojure-src/dev/ublt/emacs/clj.clj`

New source directory `clojure-src/`, added to the JVM classpath. The
namespace is a thin wrapper over `emacs_jni.Lisp`:

```clojure
(ns dev.ublt.emacs.clj
  (:import [emacs_jni Lisp LispObject]))

(defn intern [s] (Lisp/intern s))
(defn apply-async [f & args] (Lisp/applyAsync f (object-array args)))
(defn apply [f & args] (deref (Lisp/applyAsync f (object-array args))))
(defn type-of [v] (Lisp/typeOf v))
(defn is-not-nil? [v] (Lisp/isNotNil v))
(defn eq [a b] (Lisp/eq a b))
```

Loaded at runtime via `require`; no AOT step.

### Elisp

- `emacs-jni.el`: add one dispatch case to `jvm--dispatch-message`:
  `` `(lisp-request) `` → `(jvm--service-lisp-requests)`.
- `jvm-clj.el` (new library):
  - `jvm-clj-classpath` (`defvar`): list of paths added when starting
    the Clojure side — the user's Clojure jars plus the `clojure-src`
    directory.
  - `jvm-clj-start`: start the JVM if needed (honoring
    `jvm-clj-classpath`), then `(jvm-clojure-require
    "dev.ublt.emacs.clj")`. Clear error if Clojure is not reachable.
  - nREPL start/stop: thin wrappers over the existing `jvm-nrepl-start`
    / `jvm-nrepl-stop` that ensure the namespace is loaded first.

`jvm-clj.el` wraps the existing `jvm-clojure-*` / `jvm-nrepl-*`
primitives; it does not replace them.

## Testing

- ERT (mirrors `test/jvm-clojure-test.el`): drive Clojure via
  `jvm-clojure-eval-string` to exercise each fn — e.g.
  `(dev.ublt.emacs.clj/apply "+" 1 2)` yields a `LispObject` whose
  `prin1ToString` is `"3"`; `type-of`, `eq`, `is-not-nil?`; and a
  reentrancy test where Clojure calls Lisp back inside a
  Lisp→Clojure call.
- Rust unit tests for the request queue and the
  box/borrow/free pointer discipline.
- GC-driven `FreeRef` is inherently flaky to assert (forcing GC),
  same as the existing `ReleaseNotifier` — documented, not asserted.

## Risks and tradeoffs

- **Main-thread starvation**: a sync op blocks its JVM thread until
  Emacs pumps. Mitigated by a JVM-side timeout; inherent to Emacs's
  single-threaded model.
- **Pointer discipline**: `Box::into_raw`/`from_raw` around
  `GlobalRef`. Contained to the create/free paths; ops only borrow.
  Cleaner-runs-once, live-object-during-call, and the FIFO ordering
  invariant (see "Ordering invariant") keep it sound.
- **No cross-runtime cycle collection**: unchanged from the existing
  design (see `docs/explanation/interop-design.md`, "The no-cycles
  assumption"). A `LispObject` held by Clojure that transitively keeps
  a Lisp value referencing that same Clojure object would leak.
