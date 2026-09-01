# Handoff note

Written 2026-09-01, after Phase 1 shipped to `master`. For whoever (agent
or human) picks this project up next.

## Where things stand

Phase 1 (JVM lifecycle + jar loading) is done, merged to `master`, all
tests green. Read [`README.md`](README.md) first for what it does and how
to try it — this note is about *process* and *what's next*, not usage.

Working conventions for this repo:
- **No git worktrees.** This is solo local prototyping with no remote
  configured yet. Work directly on `master`.
- Every phase gets a spec → plan → implementation cycle, following the
  `superpowers` skill set (`brainstorming` → `writing-plans` →
  `subagent-driven-development`). Phase 1's artifacts are the reference:
  - Spec: `docs/superpowers/specs/2026-08-31-jvm-lifecycle-design.md`
  - Plan: `docs/superpowers/plans/2026-08-31-jvm-lifecycle-plan.md`
  - The plan's "Design note" section documents a real architecture
    correction made *during planning*, after verifying the actual jni-rs
    0.22.4 API against a compiled probe rather than trusting docs — worth
    reading before assuming any pattern from the spec carries forward
    unmodified.
- The `subagent-driven-development` process (fresh implementer per task +
  task review + final whole-branch review) caught two real cross-task bugs
  during Phase 1 that a less structured approach likely would have missed:
  a status-desync bug baked into the plan's own literal Elisp snippets
  (present identically in 3 places), and a test-isolation bug from Emacs
  batch-mode sharing process state across test files. Both fixed and
  verified. Worth continuing to use this process for Phase 2/3, not just
  because it worked but because both bugs were *cross-task* — exactly the
  class of thing a single task's own review can't see.

## Known rough edges (not blockers, just things to know)

- `jvm-stop` is best-effort (`JNI_DestroyJavaVM` reliability varies by JVM
  build) and restart-after-stop/crash relies on an internal `jni` 0.22.4
  behavior (a process-global `OnceLock` cache of the `JavaVM` pointer), not
  a real reattachment — see the comment above `create_jvm` in
  `src/jvm_thread.rs`.
- Two low-priority findings from the final review were deliberately **not**
  fixed (see the plan's git history / commit messages for the reasoning
  if you want the full rationale — the SDD ledger itself was deleted after
  merge per convention, so git log + this note are the record now):
  - A defensive-but-currently-unreachable status-desync path in
    `jvm-pause`/`jvm-stop`'s Rust defuns (only matters if a future phase
    adds a way to kill the worker thread out from under a live status —
    nothing today does).
  - A cosmetic "phantom status transition" visible to `jvm-status-hook`
    observers on an illegal call (status flips to the transient state,
    then immediately reverts).
- `mise.toml`/`mise.lock` (pins JDK 26) have been untracked since before
  Phase 1 started — nobody has decided whether to commit them. Ask before
  changing that.

## Signal handling and stdio (investigated 2026-09-01)

The embedded JVM shares Emacs's process, which has two consequences worth
knowing about before Phase 2 touches either of these areas:

**Signals.** By default HotSpot installs its own handlers for
SIGINT/SIGTERM/SIGHUP/SIGQUIT (to support `Runtime.addShutdownHook` and a
SIGQUIT-triggered thread dump) — since we're embedded, not a subprocess,
this **replaces** whatever Emacs itself does with those signals, which is
why `C-g` used to terminate Emacs outright. Fixed: `jvm-options` now
defaults to `("-Xrs")`, which stops the JVM from installing handlers for
that whole group of four signals — verified empirically (not just from
docs) with a `sigaction`-based probe during development: `SIGINT`'s handler
address changed from `0x0` to a real function pointer without `-Xrs`, and
stayed `0x0` with it. No downside for us: `jvm-stop` always tears down via
the JNI API (`JavaVM::destroy`), never via a signal, and shutdown hooks
still run on that path regardless of `-Xrs`.

Two things that follow from this, relevant if Phase 2 (or anyone) wants
"send a signal to the JVM" or a SIGQUIT-style thread dump:
- `-Xrs` disables the *whole group* of four signals together — there's no
  documented way (as of this investigation) to keep just SIGQUIT's
  thread-dump handler while still protecting SIGINT. You can't have both
  "`C-g` is safe" and "`kill -QUIT`-style thread dump via a raw signal"
  at the same time with `-Xrs` alone.
- Since the JVM is in-process, `raise()`/`kill(getpid(), ...)` from Rust is
  process-wide — it would signal Emacs too, not just "the JVM". The
  embedded-appropriate way to get signal-like effects (thread dump, GC,
  shutdown) is calling the actual Java API directly (`Thread.getAllStackTraces()`,
  `System.gc()`, `System.exit()`) via JNI, once Phase 2's "Lisp calls a JVM
  method" machinery exists — not reaching for OS signals at all.

**stdio.** `System.out`/`System.err` are just `PrintStream`s wrapping fds
1/2 — the *same* file descriptors Emacs already has open, not a takeover.
JVM-internal native diagnostics (startup banners, a SIGQUIT thread dump)
write straight to those fds too, not just Java-level `System.out`. So
everything currently interleaves on whatever Emacs's own stdout/stderr
already point to. Two ways to get Lisp-side control, if this ever matters:
- `System.setOut`/`System.setErr` — Java-level only, misses native/JVM
  output. Needs a custom `PrintStream`/`OutputStream` Java object whose
  `write()` calls back into Rust — i.e. a Lisp-callback wrapped as a JVM
  object, which is literally one of Phase 2's own deliverables ("wrap a
  Lisp value as a JVM object"), so this falls out naturally once that
  exists rather than needing its own design.
- OS-level `dup2` on fd 1/2 before `JavaVM::new()` — catches everything
  including native output, and would extend the exact `open_channel`
  pipe-process pattern Phase 1 already built (a `*jvm-stdout*`/
  `*jvm-stderr*` buffer). The catch: `dup2` on fd 1/2 is process-wide, so
  it would also swallow Emacs's *own* stdout/stderr, not just the JVM's —
  don't build this without scoping it carefully (stderr-only and opt-in
  is the safer starting point, not a default).

## What's next

**Phase 2: bidirectional JVM ↔ Lisp interop.** Per the original project
scope: wrap a Lisp value as a JVM object (needs a class/interface on the
JVM side), wrap a JVM object as a Lisp value, JVM calls a Lisp function,
Lisp calls a JVM method. Assumes no cycles in the cross-runtime object
graph (stated as an explicit simplifying assumption from the start — worth
re-confirming it still holds before designing around it).

This has NOT been brainstormed yet. Start there — `superpowers:brainstorming`,
architectural path (it's a new subsystem, not a bounded change to existing
code). Don't assume Phase 1's exact threading model maps cleanly onto
"JVM calls a Lisp function" — that direction crosses back onto the Lisp
main thread from the JVM worker thread, which is a genuinely different
problem from anything Phase 1 had to solve (Phase 1 only ever needed
Lisp-thread → worker-thread commands and worker-thread → Lisp-thread
*notifications*, never a worker-thread → Lisp-thread *call with a return
value*). That's the crux of Phase 2's design work.

**Phase 3 (after Phase 2): Clojure eval + nREPL server.** Not started, not
yet designed at all.
