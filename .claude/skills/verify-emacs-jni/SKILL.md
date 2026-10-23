---
name: verify-emacs-jni
description: Drive emacs-jni the way a user does — a real interactive Emacs daemon that loads this checkout's `jvm`/`jvm-clj` package and debug module — and capture proof. Use to prove a change to the Lisp API, the Rust module, the Java shim, or the Clojure bridge works end to end (JVM lifecycle, Java calls, jar loading, Lisp callbacks, Clojure eval, nREPL), beyond what the batch ERT tests show.
---

# Verify emacs-jni

The user surface is the Lisp API (`elisp/jvm.el`, `elisp/jvm-clj.el`) inside a running Emacs, plus
an nREPL client that connects to the embedded JVM. ERT tests run in `emacs --batch`, where Emacs
never goes idle. This harness runs a real Emacs daemon: between commands the daemon is idle, so
the wakeup pipe drains the inbound queue as it does for a user. `-async` callbacks and Clojure→Lisp
requests from nREPL threads take that path.

All commands go through one helper. Run it from the repo root:

```sh
V=.claude/skills/verify-emacs-jni/scripts/verify-emacs
```

Platform: Linux and macOS. Windows is not covered.

## Launch

```sh
VERIFY_RUN_ID=my-change $V launch
```

`launch` runs `mise run fixtures` (debug module and `target/test-fixtures/{greeter,calc}.jar`),
then starts `emacs -Q --daemon=<socket> -l dev-paths.el`. It is ready when it prints
`launched run '<id>': pid N, socket ...`: the daemon has loaded `jvm`, `jvm-clj`, and the module.
The JVM is not started yet (`jvm-status` is `not-started`).

- State: `target/verify/<id>/` (pid file, `emacs.log` with daemon stdout/stderr).
- Socket: `$XDG_RUNTIME_DIR/emacs-jni-verify/<id>/server` (else `~/.cache/...`). Not in `target/`,
  because a virtiofs checkout cannot hold Unix sockets.
- Isolation: each `VERIFY_RUN_ID` has its own daemon, socket, and state. Use a new ID for each
  concurrent run. `launch` refuses an ID that already has a live daemon.
- Set `VERIFY_RUN_ID` for every command of the run. The default ID is `default`.

The JVM is a per-process singleton, and `jvm-stop` is final for the process. To start over after
`jvm-stop` or a crash, `cleanup` and `launch` again.

## Doctor

```sh
$V doctor
```

Read-only. Fails unless the pid file's process owns the socket, `jvm-ping` answers `t`, and the
daemon loaded the module from this checkout's `target/debug/` and `jvm.el` from `elisp/`. Prints
the commit (with `+uncommitted` if `src elisp java clojure` differ from `HEAD`), the Emacs version,
and `jvm-status`. Run it first, and again whenever a result looks wrong.

The daemon does not reload changed code. After you rebuild, `cleanup` and `launch`.

## Drive

```sh
$V eval '(jvm-start)'                                         # => ready
$V eval '(jvm-call "java.lang.Integer" "toHexString" (list 255))'   # => "ff"
$V nrepl 40093 '(+ 1 2)'                                      # => value: 3 / status: ("done")
$V log                                                        # *jvm-log* contents
```

- `eval FORM` runs `emacsclient --eval FORM`. It prints the value with `prin1` (strings in quotes)
  and exits 0. A Lisp signal prints `*ERROR*: <condition message>: <data>` and exits 1.
- `nrepl PORT CODE` sends one nREPL `eval` (`scripts/nrepl-eval.clj`, run with `bb`). It prints
  `value:`, `out:`, `err:`, `ex:` lines and a final `status:` line. It exits 1 on `ex` or
  `eval-error`.
- Put forms in single shell quotes. Do not use `'` in a form: write `(list ...)` and `(quote x)`.
- `eval` blocks the daemon until the form returns. To see an `-async` result, store it in a
  variable in the callback, then poll it with further `eval` calls. The daemon drains the queue
  between calls.

The feature map is in [`features/README.md`](features/README.md). Use the matching feature file as
the recipe. A proof that drives one entry point is incomplete if the map lists others for the
changed behavior.

## Evidence

Every `eval` and `nrepl` call appends the command, its output, and its exit code to
`target/verify-evidence/<id>/transcript.txt`. `cleanup` keeps this directory. Add other
artifacts (for example `$V log > target/verify-evidence/<id>/jvm-log.txt`) to the same directory.

Proof standards:

- Use the public API (`jvm-*`, `jvm-clj-*`, `emacs-jni.clj`), not `jvm--*` internals, except
  where a feature file names one.
- Capture the action and the resulting state. For a mutation, read the state back with a second
  call (for example, call a class after `jvm-load-jar`, read a variable that a callback set).
- For a call from Clojure into Lisp, prove the Lisp side changed: read it back with `eval`.
- Record the run ID, the commit from `doctor`, and the feature ID with each claim.
- If a precondition is missing, report the journey as failed with the reason. Do not weaken the
  journey.

## Cleanup

```sh
VERIFY_RUN_ID=my-change $V cleanup
```

Sends `(kill-emacs)` to the run's daemon. If it is still alive after 10 s, kills it by its recorded
pid only. Deletes `target/verify/<id>/` and the socket directory. Keeps
`target/verify-evidence/<id>/`. Run cleanup after every failed attempt too.

## Helpers

- `scripts/verify-emacs`: `launch | doctor | eval FORM | nrepl PORT CODE | log | cleanup`.
- `scripts/nrepl-eval.clj`: `bb scripts/nrepl-eval.clj PORT CODE`. `verify-emacs nrepl` calls it.
