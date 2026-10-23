# Lifecycle

The user starts the embedded JVM, pauses it to detach the worker thread while it keeps its state,
resumes it, and stops it. Each transition runs `jvm-status-hook`. An operation that the current
status does not allow signals `jvm-illegal-transition` and leaves the status unchanged.

## Sub-features

- `lifecycle-start` starts the JVM: `not-started` → `starting` → `ready`.
- `lifecycle-pause` detaches the worker: `ready` → `pausing` → `paused`.
- `lifecycle-resume` restarts a paused JVM with its state: `paused` → `starting` → `ready`.
- `lifecycle-stop` destroys the JVM: `ready` or `paused` → `stopping` → `stopped`.
- `lifecycle-hook` runs `jvm-status-hook` once per transition, transient states included.
- `lifecycle-illegal` rejects calls and transitions that the status does not allow.

## How to get to it (user POV)

- `M-: (jvm-start)`, `(jvm-pause)`, `(jvm-stop)`: the `-await` forms.
- `jvm-start-async`, `jvm-pause-async`, `jvm-stop-async`, then wait for `jvm-status-hook`.
- `(jvm-status)` reads the status at any time.

## Driving it with verify-emacs

Preconditions:

- Baseline preconditions from the index. `jvm-status` is `not-started`.
- `calc.jar` is not needed.

- **Record transitions.** Run
  `$V eval '(progn (setq v--statuses nil) (add-hook (quote jvm-status-hook) (lambda (s) (push s v--statuses))) t)'`.
  Prints `t`.
- **Start.** Run `$V eval '(jvm-start)'`. Prints `ready`.
- **Pause.** Run `$V eval '(jvm-pause)'`. Prints `paused`.
- **Call while paused.** Run `$V eval '(jvm-call "java.lang.Math" "abs" (list -3))'`. Exit 1 with
  `*ERROR*: JVM operation not allowed in the current state: "paused"`.
- **Resume.** Run `$V eval '(jvm-start)'`. Prints `ready`. Then
  `$V eval '(jvm-call "java.lang.Math" "abs" (list -3))'` prints `3`.
- **Stop.** Run `$V eval '(jvm-stop)'`. Prints `stopped`.
- **Hook order.** Run `$V eval '(reverse v--statuses)'`. Prints
  `(starting ready pausing paused starting ready stopping stopped)`.
- **Illegal after stop.** Run `$V eval '(jvm-pause)'`. Exit 1 with
  `*ERROR*: JVM operation not allowed in the current state: "stopped"`. Then
  `$V eval '(jvm-status)'` prints `stopped`.
- **Proof.** Run `$V log > target/verify-evidence/$VERIFY_RUN_ID/jvm-log.txt`. A clean run has an
  empty log. The transcript holds the status sequence.

## Gotchas

- `jvm-stop` is final for the daemon. A later `jvm-start` is allowed, but the JVM does not work
  (see `docs/explanation/limitations-and-tradeoffs.md`). `cleanup` and `launch` to continue.
- `jvm-start` on a paused JVM ignores `jvm-classpath` and `jvm-options`.
- A failed `jvm-pause` stays in `pausing`, and `jvm-pause` signals `jvm-timeout` after 30 s. Read
  `$V log` for the cause.
- `start-failed`, `stop-failed`, and `crashed` write the detail to `*jvm-log*`. Read `$V log`
  before you report them.
