# Handoff note

Written 2026-09-02, after Phase 3 shipped to `master`. For whoever (agent
or human) picks this project up next.

## Where things stand

Phase 3 (Clojure evaluation + nREPL server) is done, merged to `master`,
all tests green (`cargo test --lib -- --test-threads=1`: 34 passed; full
ERT suite with `EMACS_JNI_TEST_FEATURES=test-support` and
`EMACS_JNI_CLOJURE_JARS_DIR` set: 11 files, 28 tests, 0 unexpected — 5 of
those in `test/jvm-clojure-test.el`). A manual smoke test also confirmed
the interactive path end to end: real `emacs -Q -L elisp`, `jvm-start-sync`,
loading the four Clojure/nREPL jars, `jvm-clojure-eval-string`, and
`jvm-nrepl-start`, then a real external nREPL client (`nc`, a genuinely
separate OS process, distinct from the automated test's `bash`+`/dev/tcp`)
connecting and evaluating `(+ 100 23)` over real bencode — got back
`5:value3:123` and `status: done`. See [README.md](README.md)'s new
"Clojure & nREPL" section for the exact commands. Read `README.md` first
for what it does and how to try it — this note is about *process* and
*what's next*.

The original three-phase scope for this project (JVM lifecycle → JVM↔Lisp
interop → Clojure/nREPL) is now **complete**.

What shipped:
- `jvm-clojure-require (namespace &optional timeout)` and
  `jvm-clojure-eval-string (code &optional timeout)` — thin Elisp sugar
  over `Clojure.var`/`Clojure.read` + `jvm-call-sync`, no new Rust
  primitives.
- `jvm-nrepl-start (&optional port timeout)` / `jvm-nrepl-stop (server
  &optional timeout)` — same pattern, chaining `jvm-call-sync` calls to
  `nrepl.server/start-server` and `stop-server`.
- All four built entirely on Phase 2's existing `jvm-call`/`jvm-new`
  machinery — no new wire messages, no new `Cmd` variants, no new
  marshalling.
- `test/fetch-clojure-jars.sh` and `test/jvm-clojure-test.el` (5 tests,
  gated behind `EMACS_JNI_CLOJURE_JARS_DIR`, skip cleanly when unset).

**The one real Rust-level surprise this phase needed** (same spirit as
Phase 1's `OnceLock` finding and Phase 2's `Global<T>`/`FindClass`
findings): a freshly spawned `Cmd::Call` thread has a **null context
classloader**. Plain JNI class resolution (`find_class`/`loadClass`,
already used by every existing `jvm-call`) doesn't need one, but
Clojure's own namespace loader (`clojure.lang.RT`, used internally by
`require`) does — it loads `.clj` source files as classpath *resources*,
not classes, and that resolution path falls back through
`Thread.currentThread().getContextClassLoader()`. Left null, the first
touch of Clojure's runtime threw `ExceptionInInitializerError` before
`clojure.core` even finished loading. Fixed in `handle_call`
(`src/jvm_thread.rs`): set the spawned thread's context classloader to
the shared jar-loading `URLClassLoader` (when one exists), once, right
after `attach_current_thread` and before any reflective work — a cheap,
one-time JNI call, harmless for calls that never touch Clojure. Verified
load-bearing by reverting it and confirming the new test failed, then
restoring it and confirming the test passed again (see
`.superpowers/sdd/2026-09-02-clojure-nrepl-plan/task-1-report.md`). This
was found and fixed during a pre-planning empirical spike, not guessed at
in the spec — see the design spec's "Feasibility" section for the full
account, including how it was diagnosed.

No other Rust code changed this phase. Everything else — the Elisp
convenience layer — is new Lisp on top of existing, unmodified
primitives.

## Known rough edges

Not blockers, but real, current gaps worth knowing before building on top
of this.

**New this phase:**

- **`*ns*` does not persist across separate `jvm-clojure-eval-string`
  calls.** Each call is its own `jvm-call`, which dispatches to a freshly
  spawned OS thread (Phase 2's `Cmd::Call` design); Clojure's
  current-namespace pointer is thread-local, so a fresh thread sees the
  *root* binding, not whatever a previous eval's `(in-ns ...)` set on some
  other, now-discarded thread. `(def x 5)` then `x` in a later call does
  work (`def` mutates a genuinely global `Var`); relying on a prior
  `in-ns` does not. Workaround: fold namespace switches into the same
  eval string. Does **not** affect the nREPL server — once
  `jvm-nrepl-start` has returned, all eval activity for a client session
  runs inside nREPL's own session/thread management, which handles
  per-session namespace state correctly and independently of `Cmd::Call`.
  See the design spec's "Known limitation" section and
  `jvm-clojure-eval-string`'s own docstring.
- **Non-scalar Clojure return values come back as an opaque wrapped JVM
  object.** `jvm-clojure-eval-string`'s result goes through Phase 2's
  existing marshalling unchanged: `null`/boxed numerics/`String`/
  `Boolean`/`BigInteger` marshal to the obvious Lisp value; a keyword,
  symbol, vector, map, or lazy seq is "anything else" and arrives as a
  `user-ptr`, not a Lisp-visible structure — same limitation as any other
  non-scalar `jvm-call` result. Call `.toString()` or Clojure's own
  `pr-str` via another `jvm-call` for a printable form.
- **The nREPL server is unhardened.** `jvm-nrepl-start` uses nREPL's
  defaults throughout: loopback-only usage assumed, no authentication, no
  TLS, no middleware selection. Explicitly out of scope per the design
  spec ("Out of scope": "nREPL middleware selection/configuration, TLS,
  authentication, or binding to a non-loopback interface").
- **`test/jvm-clojure-test.el`'s bencode test has no `unwind-protect`
  around its `jvm-nrepl-stop` call.** If an assertion between
  `jvm-nrepl-start` and `jvm-nrepl-stop` fails mid-test, the started nREPL
  server thread leaks for the rest of the test process (that `emacs
  --batch` process exits right after, so it doesn't outlive the test run,
  but it does run unstopped for whatever remains of it). Low impact — only
  triggers on an already-failing run — and deliberately not fixed here:
  it's plan-mandated test code from Task 3, out of scope for that task,
  and fixing it as part of this docs-only task would be undocumented scope
  creep. Worth a follow-up if anyone touches that test file again.

**Carried over from Phase 2 (still current — nothing this phase touched
the code these describe):**

- Overload resolution (`invoke_method`/`invoke_constructor` in
  `src/jvm_thread.rs`) still only falls through to the next same-arity
  candidate on a marshalling *failure*; a wrong-typed non-numeric argument
  can pass marshalling and only fail later at the actual `Method.invoke()`
  call, a failure mode the fall-through doesn't retry on.
- A panic inside a `Cmd::Call`'s per-call spawned thread still vanishes
  silently (no `catch_unwind`, no join) — a panicking call leaves a
  `-sync` caller hanging until its timeout.
- `jvm--lisp-callbacks` entries can still leak if a `jvm-call` passing a
  Lisp function fails before the JVM-side `LispFunction` proxy is
  constructed.
- Still no automated test for the GC-notification path (manually
  spot-checked only).
- `jvm-call`/`jvm-new` still trust the caller completely — reflective
  dispatch on Lisp-supplied class/method names with no access
  restriction, an accepted assumption for this embedding model.
- `mise.toml`/`mise.lock` (pins JDK 26) are still untracked. Nobody has
  decided whether to commit them. Ask before changing that.
- `cargo test --lib`'s default parallel mode is still flaky (pre-existing
  `jni`-rs race across concurrent first-time `JavaVM::new()` calls in
  different test modules). Run with `-- --test-threads=1` for a reliable
  result — that's what this task's own verification pass used.

## What's next

**The original three-phase scope for this project is complete.** There is
no mandated Phase 4. If someone picks this project up again, possibilities
worth considering (not a plan, not scoped, not designed):

- **Packaging/distribution** — right now this only runs from a source
  checkout (`cargo build` + `-L elisp`). No MELPA recipe, no prebuilt
  binaries, no bundled JDK/Clojure (deliberately, per this project's
  "point at your own JDK/jars" policy — see README's Prerequisites and
  "Clojure & nREPL" sections).
- **Richer Clojure value marshalling** — teaching `jvm-call`'s existing
  marshalling about Clojure keywords/vectors/maps specifically (rather
  than the generic opaque-handle fallback) would remove the biggest
  practical friction in `jvm-clojure-eval-string`, at the cost of picking
  a Lisp-side representation for each (keyword → interned symbol? vector →
  list or vector? lazy seq → force-and-convert, or leave lazy?) — a real
  design question, not a bounded addition.
- Addressing any of the "Known rough edges" above, Phase-2-carried or
  new, if a real need for one surfaces.
