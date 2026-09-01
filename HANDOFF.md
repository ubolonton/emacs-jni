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
