# Phase 3 Design: Clojure Evaluation and nREPL Server

Status: approved for planning. Written and self-approved autonomously
(user unavailable during this session; explicitly authorized full
autonomous operation for this phase, "use your own judgment") — every
design decision below carries its rationale so it can be reviewed and
overturned later if wrong, the same way earlier design notes in this
project record decisions made under empirical verification rather than
live back-and-forth.

Date: 2026-09-02

## Purpose

Phase 3's original scope (per the project's three-phase decomposition):
evaluate Clojure code, and run an nREPL server other processes can
connect to. This document covers both, built entirely on top of Phase 2's
already-shipped `jvm-call`/`jvm-new` machinery.

## Feasibility: verified empirically before designing further

Before writing this spec, the central architectural question HANDOFF.md
raised after Phase 2 — "does `Cmd::Call`'s one-thread-per-call model, and
`invoke`'s call-completes-and-returns model, actually fit a long-lived,
stateful, socket-driven nREPL session?" — was tested against a real
Clojure runtime and a real nREPL server, not reasoned about from
documentation. Using `curl`-fetched jars (`org.clojure:clojure:1.12.0` +
its `spec.alpha`/`core.specs.alpha` dependencies from Maven Central,
`nrepl:nrepl:1.7.0` from Clojars — nREPL is published to Clojars, not
Maven Central, discovered during this verification) and nothing but
existing `jvm-load-jar-sync`/`jvm-call-sync` calls from a batch Emacs
process, the spike:

1. Loaded Clojure + nREPL into the JVM via the existing shared classloader
   (Phase 1/2 machinery, unmodified).
2. Bootstrapped Clojure via its own embedding-facade class,
   `clojure.java.api.Clojure` (`var(ns, name)` → `IFn`, `read(string)` →
   parsed Clojure data) — both plain reflective calls, already fully
   supported by `jvm-call`.
3. Called `(require 'nrepl.server)` and `(start-server :port 0)` purely
   via chained `jvm-call-sync` calls.
4. Confirmed `start-server` returns quickly without blocking the worker's
   dispatch loop (verified by issuing an unrelated `jvm-call` immediately
   after and confirming it completed normally) — because `start-server`
   itself is a one-shot call, exactly like every other `jvm-call` Phase 2
   already handles; the actual long-lived socket-accept loop runs on
   **threads the JVM itself spawns** inside `start-server`, entirely
   outside `Cmd::Call`'s per-call-thread lifecycle. HANDOFF.md's concern
   doesn't materialize: nothing about Phase 2's threading model needs to
   change.
5. Connected to the bound port from a **separate OS process** (plain
   `bash` + `/dev/tcp`, deliberately not reusing any Emacs/Lisp networking
   code, to prove genuine external reachability) and exchanged real
   bencode-encoded nREPL protocol messages: a `describe` op returned the
   server's real middleware stack (879 bytes), and an `eval` op for
   `(+ 40 (* 2 1))` returned a real, correct result: `value` `"42"`,
   `ns` `"user"`, a session id, `status` `["done"]`.

One real bug was found and confirmed fixed during this process, not
assumed: a freshly spawned `Cmd::Call` thread has a **null context
classloader**. Plain JNI class resolution (`find_class`/`loadClass`,
which is what `jvm-call`'s own reflection already uses) doesn't need it —
but Clojure's own namespace loader (`clojure.lang.RT`, used internally by
`require`) does, since it loads `.clj` **source files** as classpath
*resources*, not classes, and that resolution path falls back through
`Thread.currentThread().getContextClassLoader()`. Left unset, the very
first touch of Clojure's runtime throws `ExceptionInInitializerError`
before `clojure.core` even finishes loading. Explicitly setting the
context classloader to the same shared `URLClassLoader` jar-loading
already uses, once per `Cmd::Call` thread right after attaching, resolved
it — confirmed by re-running the exact same spike after the fix and
getting a real, working nREPL round trip. See "Architecture" below for
where this lands in the codebase.

## Architecture

**No new marshalling subsystem, no new wire-protocol messages, no new
`Cmd` variants.** Phase 2's `jvm-call`/`jvm-new` plus the existing wrapped-
JVM-object `user-ptr` mechanism are sufficient to bootstrap and drive both
Clojure evaluation and the nREPL server — confirmed by the spike above,
which never touched anything below the existing `jvm-call` primitive. The
entire deliverable is:

1. **One small, targeted fix to `src/jvm_thread.rs`'s `handle_call`**: set
   the spawned thread's context classloader to the shared jar-loader
   (when one exists) immediately after `attach_current_thread`, before any
   reflective work. This is a one-time, cheap JNI call
   (`Thread.currentThread().setContextClassLoader(...)`), harmless for
   calls that never touch Clojure, and is what makes `require`-based
   namespace loading work at all. This is the only Rust code this phase
   needs.
2. **A thin Elisp convenience layer**, built entirely on `jvm-call`/
   `jvm-call-sync`/`jvm-new`, wrapping the bootstrap sequence the spike
   already proved out (`Clojure.var`/`Clojure.read` → `require` → eval or
   `start-server`) so callers don't have to hand-chain 4-5 `jvm-call`s
   themselves every time.

### Lisp-facing API

- `jvm-clojure-require (namespace &optional timeout)` — sugar for
  `(require 'NAMESPACE)`, via `Clojure.var("clojure.core", "require")` +
  `Clojure.read(namespace)`. Idempotent (Clojure's own `require` already
  is).
- `jvm-clojure-eval-string (code &optional timeout)` — sugar for
  `(eval (read-string code))`, via `Clojure.var("clojure.core", "eval")` +
  `Clojure.read(code)`. Returns whatever Phase 2's existing marshalling
  produces for the result (see "Known limitation" below for what that
  means for non-scalar Clojure values).
- `jvm-nrepl-start (&optional port timeout)` — bootstraps
  (`jvm-clojure-require "nrepl.server"`) then calls `start-server` (port 0
  → OS-assigned free port, matching the spike). Returns a cons
  `(PORT . SERVER)`, where `SERVER` is the wrapped `user-ptr` needed to
  stop it later — mirrors the existing `(OK . ERR)`-family return-shape
  convention rather than inventing a new one.
- `jvm-nrepl-stop (server &optional timeout)` — calls
  `nrepl.server/stop-server` on the retained server object.

All four are thin Elisp functions with no new Rust-level primitives
behind them — every JNI-touching step they perform is an existing
`jvm-call`/`jvm-new` call. Async/`-sync` variants follow the same
convention as `jvm-call`, but since none of these are ever called in a
hot loop (they're one-shot setup operations), only `-sync` forms are
planned for v1; add async forms later only if a real need shows up
(YAGNI — Phase 2 built both because `jvm-call` itself is a
general-purpose primitive that legitimately needs both; these four are
narrow, one-shot convenience wrappers, not a general primitive).

### Jar acquisition: user-supplied, not bundled

Same policy as the JDK itself (Phase 1: "point `JAVA_HOME` at your own
JDK, we don't bundle one"). Bundling Clojure + nREPL would mean picking
and maintaining a version pin, dealing with license/redistribution
questions, and growing the repository by several MB for something every
Clojure user already has their own preferred way to obtain (`clj`,
Leiningen, deps.edn, or — as this spike did — direct jar downloads).
Instead: document the exact jars/versions this was verified against
(`org.clojure/clojure` 1.12.0 + `org.clojure/spec.alpha` 0.5.238 +
`org.clojure/core.specs.alpha` 0.4.74 from Maven Central,
`nrepl/nrepl` 1.7.0 from **Clojars**, not Maven Central — a real gotcha
worth documenting explicitly, since assuming Maven Central for all
Clojure-ecosystem artifacts is a natural but wrong assumption), and load
them the same way any other user jar is loaded: `jvm-classpath` or
`jvm-load-jar`.

### Known limitation: `*ns*` does not persist across separate `jvm-clojure-eval-string` calls

Each `jvm-call` (including the one `jvm-clojure-eval-string` makes)
dispatches to its own freshly spawned OS thread (Phase 2's `Cmd::Call`
design). Clojure's current-namespace pointer (`*ns*`) is a thread-local
dynamic binding; a fresh thread sees its *root* binding, not whatever a
previous eval's `(in-ns ...)` may have set on some other, now-discarded
thread. Concretely: `(jvm-clojure-eval-string "(def x 5)")` followed by
`(jvm-clojure-eval-string "x")` **does** work (`def` mutates a namespace-
global `Var`, which is genuinely global JVM state, not thread-local), but
`(jvm-clojure-eval-string "(in-ns 'foo)")` followed by a second call
relying on still being in `foo` **does not** — the second call starts
back at the root namespace. Workaround: fold `(in-ns ...)`/`binding` into
the *same* eval string when it matters, rather than relying on it
persisting across calls.

This limitation does **not** apply to the nREPL server itself. Once
`jvm-nrepl-start` has returned, all subsequent eval activity for a given
nREPL client session happens entirely inside nREPL's own request-handling
machinery (external client → socket → nREPL's own thread/session
management → eval), which manages its own per-session namespace state
correctly and independently of `Cmd::Call`'s threading — confirmed by the
spike's `eval` response correctly reporting `ns` `"user"` and a stable
`session` id. `jvm-clojure-eval-string` is a narrower, ad hoc convenience
for quick one-off evals from Lisp directly; real interactive Clojure work
is expected to go through the nREPL server and a real nREPL client, where
this limitation doesn't exist.

### Known limitation: non-scalar Clojure return values

`jvm-clojure-eval-string`'s result goes through Phase 2's existing
marshalling unchanged: `null` → `nil`, boxed numerics/`String`/`Boolean`/
`BigInteger` → the obvious Lisp value, anything else → a wrapped
`user-ptr`. A Clojure keyword, symbol, vector, map, or lazy sequence is
"anything else" — it comes back as an opaque handle, not a Lisp-visible
structure. This mirrors Phase 2's own accepted cut of "no automatic Lisp
list/vector ↔ Java `List`/array marshalling" for the same reason
(genuinely ambiguous, not needed for this phase's actual deliverable — an
nREPL server, whose clients get real Clojure-native prints over the wire
protocol itself, never through this path). If a printable form is needed
from Lisp directly, call `.toString()` or Clojure's own `pr-str` on the
result via another `jvm-call`, the same way any other opaque wrapped
object is inspected today.

## Error handling

No new mechanism. A Clojure exception during eval (whether via
`jvm-clojure-eval-string` or as part of the bootstrap sequence) is a Java
exception thrown from a reflective call, already handled by Phase 2's
existing `Cmd::Call` error path (`call-done`'s `err` field, surfaced as
`(nil . ERR-STRING)`-shaped output by the `-sync` wrappers). Nothing
Clojure-specific needs adding.

## Testing

- A unit-level Rust test is not practical here (Clojure isn't vendored in
  the crate, and downloading it during `cargo test` would make the test
  suite depend on network access) — matches this phase's "user-supplied
  jars" policy. Instead: ERT integration tests, gated behind an opt-in
  environment variable pointing at a local Clojure+nREPL jar directory
  (mirroring `EMACS_JNI_TEST_FEATURES=test-support`'s opt-in pattern),
  skipped cleanly (not failed) when unset.
- Test coverage should include, at minimum: `jvm-clojure-require` on a
  real namespace, `jvm-clojure-eval-string` round-tripping a scalar result
  (matching the spike's `(+ 40 (* 2 1))` → `42`), `jvm-nrepl-start`
  returning a real bound port and `jvm-nrepl-stop` tearing it down
  cleanly, and — the one test that actually proves the deliverable — a
  real external socket connection (the same `/dev/tcp`-style raw bencode
  exchange the spike used, or an Emacs Lisp `open-network-stream`-based
  version once that's debugged; the spike's own Elisp-side connection
  attempt failed even though the *server* was proven working via a
  separate shell process, so this needs its own debugging pass during
  implementation, not assumed to be identical to the shell-based version
  that worked) confirming a `describe` or `eval` op gets a real bencode
  response.
- Document the exact jar-fetching commands (verified working during this
  spike) in the test setup instructions, mirroring how
  `test/run-ert-tests.sh` already documents building the `Greeter`/`Calc`
  fixture jars.

## Out of scope

- Bundling Clojure/nREPL jars with the repository (see "Jar acquisition").
- Fixing `*ns*` non-persistence across direct `jvm-clojure-eval-string`
  calls (documented limitation with a workaround; the nREPL server path
  doesn't have this problem, and that's the primary deliverable).
- Any Clojure-specific value marshalling beyond what Phase 2 already
  provides (keywords, vectors, maps arriving as opaque handles is
  accepted, matching Phase 2's own collection-marshalling cut).
- nREPL middleware selection/configuration, TLS, authentication, or
  binding to a non-loopback interface — the spike used defaults
  throughout; hardening the server for anything beyond local development
  use is a different, later concern if it ever becomes one.
