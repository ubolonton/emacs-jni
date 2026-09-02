# Handoff note

Written 2026-09-02, after Phase 2 shipped to `master`. For whoever (agent
or human) picks this project up next.

## Where things stand

Phase 2 (bidirectional JVM ↔ Lisp interop) is done, merged to `master`,
all tests green (`cargo test --lib`: 34 passed; full ERT suite: 10 files,
23 tests, 0 unexpected). A manual smoke test also confirmed the
interactive path works end to end (real `emacs -Q -L elisp`, not just the
automated suite) — see [README.md](README.md)'s Interop example for the
exact commands. Read `README.md` first for what it does and how to try
it — this note is about *process* and *what's next*.

What shipped:
- `jvm-call`/`jvm-call-sync` and `jvm-new`/`jvm-new-sync`: reflection-based
  dispatch on a Lisp-supplied class name, method/constructor name, and
  argument list.
- Type-directed marshalling in both directions: numbers, bignums,
  strings, booleans, wrapped JVM objects.
- A Lisp function passed as a `jvm-call`/`jvm-new` argument is
  auto-wrapped as a real async Java object (`LispCallable`) — the JVM
  side calls it and gets a `CompletableFuture`.
- JVM-initiated calls back into Lisp (`invoke`/`InvokeReply` wire
  messages), with a real result value returned to the JVM caller, not
  fire-and-forget.
- Error propagation both directions: a Java exception becomes a Lisp
  error (with message and cause chain), a Lisp error becomes a Java
  exception.
- GC notification both directions: a JVM-side `Cleaner` sends a release
  message when a wrapped Lisp value is collected on the JVM side; Emacs
  GC on a `JvmHandle` sends a release the other way. Neither side leaks a
  cross-runtime reference under normal operation (see Known rough edges
  for the one path this doesn't cover).

Deviations from the plan found necessary during implementation (same
spirit as Phase 1's handoff documenting the `jni` `OnceLock` finding):
- `jni` 0.22.4's `Global<T>` has no `Clone` impl. The handle table stores
  `Arc<Global<JObject<'static>>>`, not a bare `Global`.
- JNI's `FindClass` can't see jar-loaded classes (it uses the caller's
  defining loader). Class lookups instead go through the shared
  `URLClassLoader#loadClass` reflectively; that loader is `Arc`-wrapped so
  `Cmd::Call`'s per-call spawned threads can share it.
- The shim classes' (`LispCallable`/`LispFunction`/`LispError`) defining
  classloader had to move from the system classloader to the platform
  classloader, so the jar `URLClassLoader` (parented on platform, not
  system) can see them.
- `shim::define_classes` was, through most of implementation, never
  actually wired into the real `create_jvm` startup path — only exercised
  by an isolated test JVM. It's now called from `create_jvm` itself: a
  genuine first-boot failure is fatal (surfaces `start-failed`), a
  failure on restart-after-stop is not (matches Phase 1's already-
  documented shared-VM-across-restart quirk).
- Overload resolution (`invoke_method`/`invoke_constructor` in
  `src/jvm_thread.rs`) implements "first same-arity candidate, fall
  through to the next on marshalling failure" rather than the spec's full
  "best arg-count/assignability match" — see Known rough edges.
- A JVM object passed as an argument *into* a Lisp callback (via
  `invoke`) needed explicit `jvm--resolve-sexp-result` wrapping on the
  Lisp side, or it arrived as a raw `(jvm-handle N)` list with a leaking
  `GlobalRef` instead of a proper wrapped user-ptr. Caught by the final
  whole-branch review, not any single task's own review — a real instance
  of the "cross-task bug only a final review catches" pattern Phase 1's
  handoff also called out.

## Known rough edges

Not blockers, but real, current gaps worth knowing before building on top
of this:

- **Overload resolution is still partial.** It falls through to the next
  same-arity candidate on a marshalling failure (fixed in the final
  review's fix wave), but `lisp_arg_to_jvalue`'s non-numeric argument arms
  (string, `nil`, `t`, wrapped object — `src/marshal.rs`) accept any
  target parameter type unconditionally, unlike the numeric arms. A
  wrong-typed non-numeric argument can pass marshalling and only fail
  later at the actual `Method.invoke()` call — a failure mode the
  fall-through doesn't retry on. Not hit by anything currently tested; a
  real gap if overload resolution needs to lean on this more heavily
  later.
- **A panic inside a `Cmd::Call`'s per-call thread vanishes silently.**
  `src/jvm_thread.rs`'s dispatch loop does `std::thread::spawn(move ||
  handle_call(...))` with no `catch_unwind` and no join. A panicking call
  leaves a `-sync` caller hanging until its timeout, with no diagnostic
  beyond whatever Rust's default panic hook prints to stderr.
- **Lisp callback-table entries can leak.** `jvm--lisp-callbacks` (in
  `elisp/emacs-jni.el`) registers a Lisp function eagerly, before sending
  the `jvm-call`. If that call fails before the JVM-side `LispFunction`
  proxy is actually constructed (e.g. no matching method found, class not
  found), the entry is never removed — the only release path is the
  JVM-side proxy being GC'd, which never happens if it was never created.
  Bounded/low-severity at prototype scale.
- **No automated test for the GC-notification path.** Both directions
  were manually spot-checked during implementation (a real JVM GC event
  triggering the Lisp-side release, confirmed end to end), but no
  `test/*.el` file exercises it — triggering a real GC deterministically
  in a short-lived batch test is the obstacle.
- **`jvm-call`/`jvm-new` trust the caller.** They do reflective dispatch
  on Lisp-supplied class and method name strings with no access
  restriction — any Lisp caller can invoke arbitrary JVM methods this way
  (e.g. `Runtime.exec`). This is an accepted assumption for this
  embedding model (Lisp code already has unrestricted access to Emacs and
  the local machine), not a defect — stating it explicitly rather than
  leaving it implicit.
- A few smaller items, parked as non-blocking during review: vestigial
  test-only scaffolding never removed once real tests existed
  (`handles::register_dummy_for_test`, `jvm--test-make-handle`);
  `LispArg::Object` and `LispArg::Callback` both render as `(jvm-handle
  N)` in `lisp_arg_to_sexp` (harmless today — `Callback` never reaches
  that function — but a latent collision if that changes); a couple of
  stale doc comments (`lisp_list_to_args` in `src/lib.rs`, describing
  pre-fix behavior).
- `mise.toml`/`mise.lock` (pins JDK 26) are still untracked, same as at
  the end of Phase 1 — nobody has decided whether to commit them. Ask
  before changing that.
- `cargo test --lib`'s default parallel mode is flaky: a pre-existing
  `jni`-rs race across concurrent first-time `JavaVM::new()` calls in
  different test modules (not introduced by Phase 2), reproduced again
  during this task's verification pass. Run `cargo test --lib --
  --test-threads=1` for a reliable full-suite result.

## What's next

**Phase 3: Clojure eval + nREPL server.** Not designed yet. Start with
`superpowers:brainstorming`, architectural path — same process Phase 2
itself went through
([spec](docs/superpowers/specs/2026-09-01-jvm-lisp-interop-design.md),
[plan](docs/superpowers/plans/2026-09-01-jvm-lisp-interop-plan.md)) — not
a bounded change to existing code.

One thing worth checking early, before assuming Phase 2's threading model
carries over unmodified: an nREPL server means the JVM side originates a
long-lived, stateful, socket-driven session, not a single call-and-return.
Phase 2's `Cmd::Call` model is one spawned thread per call, and `invoke`
assumes a call from the JVM into Lisp completes and returns — neither was
designed around a session that stays open across many eval requests.
Confirm that model still fits before building on it as-is.
