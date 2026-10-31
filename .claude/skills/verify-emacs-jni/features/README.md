# emacs-jni verification map

This directory is the maintained source for verifying the user-facing behavior of emacs-jni. Read
this index before you drive the daemon, then use the matching feature file as the recipe.

## Baseline preconditions

- `VERIFY_RUN_ID=<id> $V launch` succeeded in this run, with
  `V=.claude/skills/verify-emacs-jni/scripts/verify-emacs`.
- `$V doctor` passes and reports `jvm-status: not-started`.
- `R=$(git rev-parse --show-toplevel)` gives absolute fixture paths:
  `$R/target/test-fixtures/calc.jar` and `$R/target/test-fixtures/greeter.jar`.
- Never drive a daemon that this run did not start.

## Driving conventions

- One daemon holds one JVM. `jvm-stop` is final for it. Run the lifecycle stop journey last, or in
  its own run.
- Recipes after [Lifecycle](./lifecycle.md) assume `(jvm-start)` returned `ready`.
- Treat every command as literal. Keep quoted names unchanged.
- Name scratch variables `v--*`, so they do not collide with package variables.

## Proof and skip reporting

- The transcript (`target/verify-evidence/<id>/transcript.txt`) records each command, its output,
  and its exit code.
- Mutation proof includes a second, read-only call that shows the new state.
- Record the feature ID and the entry point (`-direct`, `-await`, `-async`, nREPL) of each claim.
- Report an entry point that you could not reach, with the command and the unmet precondition. Do
  not report it as verified through a different entry point.

## Feature entry contract

Each feature file starts with an H1 title and one paragraph that describes the user-visible
behavior. Then it has exactly four H2 sections, in this order:

1. `Sub-features`: short IDs, one line each.
2. `How to get to it (user POV)`: every user entry point.
3. `Driving it with verify-emacs`: starts with `Preconditions:`, then labeled bullets that pair
   each user action with an exact command and the observable result.
4. `Gotchas`: traps that can waste or invalidate a run.

## Features

- [Lifecycle](./lifecycle.md): start, pause, resume, stop, status hook, illegal transitions.
- [Call Java](./call-java.md): static, constructor, and instance calls on the direct, await, and
  async paths; error conditions.
- [Load a jar](./load-jar.md): add a jar to the shared classloader on each path; missing paths.
- [Lisp callbacks](./lisp-callbacks.md): pass a Lisp function to Java and get its result or signal.
- [Clojure and nREPL](./clojure-nrepl.md): start Clojure, evaluate code, serve an nREPL client
  that reads and writes Lisp state.
