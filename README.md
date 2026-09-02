# emacs-jni

An Emacs dynamic module (Rust, using the [`emacs`](https://github.com/ubolonton/emacs-module-rs)
crate) that embeds a JVM via [`jni`](https://github.com/jni-rs/jni-rs).

**Status: Phase 1, 2, and 3 done.** JVM lifecycle control
(start/pause/stop/status), jar loading, bidirectional JVM ↔ Lisp
interop: call a JVM method or constructor from Lisp (`jvm-call`/
`jvm-new`), pass a Lisp function as an argument (the JVM side calls it
back and gets a real result), full error propagation both directions,
Clojure evaluation, and a real nREPL server other processes can connect
to. See [Roadmap](#roadmap).

**Full documentation, in tutorial, how-to, reference, and explanation
form:** [`docs/index.md`](docs/index.md).

Design rationale from development itself:
[`docs/superpowers/specs/2026-08-31-jvm-lifecycle-design.md`](docs/superpowers/specs/2026-08-31-jvm-lifecycle-design.md)
(Phase 1),
[`docs/superpowers/specs/2026-09-01-jvm-lisp-interop-design.md`](docs/superpowers/specs/2026-09-01-jvm-lisp-interop-design.md)
(Phase 2), and
[`docs/superpowers/specs/2026-09-02-clojure-nrepl-design.md`](docs/superpowers/specs/2026-09-02-clojure-nrepl-design.md)
(Phase 3).

## Prerequisites

- Emacs 28+ (this repo has been exercised against Emacs 30.1). Needs
  `Env::open_channel`, added in Emacs 28.
- Rust (stable toolchain; `cargo build` picks up whatever's on `PATH`).
- A JDK. This repo pins JDK 26 via [mise](https://mise.jdx.dev/)
  (`mise.toml`); `jni`'s `invocation` feature locates `libjvm.so` via
  `JAVA_HOME` (or `java` on `PATH`) at runtime, so make sure one of those
  points at your JDK.
- Linux only for now (macOS/Windows are explicitly out of scope for Phase 1).

## Build

```sh
cd emacs-jni
cargo build
```

This produces `target/debug/libemacs_jni.so`, which `elisp/emacs-jni.el`
loads automatically (relative to its own location, or from
`$EMACS_JNI_MODULE_PATH` if you set it — see `test/run-ert-tests.sh` for how
the test suite uses that).

## Quickstart

This walks through building a real jar (the same fixture the test suite
uses) and loading it, using only commands you can copy-paste from the repo
root.

**1. Build the module and a tiny test jar:**

```sh
cargo build
mkdir -p target/test-fixtures
javac -d target/test-fixtures test/fixtures/Greeter.java
jar cf target/test-fixtures/greeter.jar -C target/test-fixtures Greeter.class
```

`test/fixtures/Greeter.java` is a one-method class already in this repo:

```java
public class Greeter {
    public static String greet() {
        return "hello from Greeter";
    }
}
```

**2. Launch Emacs with the module on the load path, from the repo root:**

```sh
emacs -Q -L elisp
```

(`-Q` skips your personal init file so this stays reproducible; `-L elisp`
puts `elisp/emacs-jni.el` on `load-path`.)

**3. In `*scratch*` (or `M-x ielm`), evaluate one form at a time:**

```elisp
(require 'emacs-jni)

(jvm-start-sync)
;; => ready   (blocks until the JVM is up; ~40ms in practice)

(jvm-status)
;; => ready

(jvm-load-jar-sync (expand-file-name "target/test-fixtures/greeter.jar"))
;; => (t)   -- a cons (OK . ERR); nil ERR prints as nothing after OK, so success is "(t)"

(jvm-load-jar-sync (expand-file-name "target/test-fixtures/does-not-exist.jar"))
;; => (nil . "jar file not found: /path/to/your/checkout/emacs-jni/target/test-fixtures/does-not-exist.jar")
;;    (the exact path is wherever `expand-file-name' resolves it in your checkout)
```

At this point `Greeter` is loaded into the JVM's shared classloader. This
walkthrough stops at jar loading; see [Interop example](#interop-example)
below for calling a method (`(jvm-call-sync "Greeter" "greet" nil)` works
here too, returning `(t "hello from Greeter" nil)`). If you just want to
confirm the class is really loadable and callable end-to-end without
Lisp, that's what
`cargo test --lib jvm_thread::tests::add_jar_url_makes_the_jars_class_loadable_and_callable`
does on the Rust side (it reflectively calls `Greeter.greet()` and asserts
the returned string).

**4. Pause and resume the same JVM:**

```elisp
(jvm-pause-sync)
;; => paused

(jvm-start-sync)
;; => ready   -- resumes the *same* JVM (jars stay loaded); does not re-run JVM init

(jvm-status)
;; => ready
```

**5. Stop (experimental — see [Known limitations](#known-limitations)):**

```elisp
(jvm-stop-sync)
;; => stopped, or stop-failed -- both are "the operation completed", see below
```

**6. Seeing a real failure and the log buffer, in a *fresh* Emacs session.**
`jvm-classpath`/`jvm-options` are only honored on a genuinely fresh start
(see the API reference below), so this needs its own new `emacs -Q -L
elisp` session, not a continuation of steps 1-5 — the JVM there is already
past `not-started`, so a second `jvm-start-sync` would just hit an illegal-transition error instead of actually retrying JVM creation with new
options. In the fresh session:

```elisp
(require 'emacs-jni)

(condition-case err
    (jvm-start-sync nil '("-Xnotarealoption"))
  (error (message "start-sync signaled: %S" err)))

(jvm-status)
;; => start-failed

(with-current-buffer "*jvm-log*" (buffer-string))
;; => "[status] start-failed: JNI call failed\n"
```

`*jvm-log*` only receives `start-failed`/`stop-failed`/`crashed` status
messages and the rare pause-detach-failure `log` line — it is *not* where
`jvm-load-jar` failures go (those come back directly in the return value,
as in step 3 above). The underlying JVM's own diagnostic (e.g.
`Unrecognized option: -Xnotarealoption`) goes to Emacs's stderr from the
JVM process itself, not into this Lisp-visible string — `*jvm-log*` tells
you *that* it failed, not always *why* in detail.

## Interop example

Calling a JVM method from Lisp, including passing a Lisp function as an
argument the JVM calls back into.

**1. Build a small fixture class and jar.** `test/fixtures/Calc.java` is
already in this repo. It needs the shim classes on its classpath, which
`cargo build` produces at `target/emacs-jni-shim-classes` (via `build.rs`):

```sh
cargo build
mkdir -p target/test-fixtures
javac -cp target/emacs-jni-shim-classes -d target/test-fixtures test/fixtures/Calc.java
jar cf target/test-fixtures/calc.jar -C target/test-fixtures Calc.class
```

**2. Launch Emacs, same as the Quickstart above:**

```sh
emacs -Q -L elisp
```

**3. In `*scratch*`, evaluate one form at a time:**

```elisp
(require 'emacs-jni)

(jvm-start-sync)
;; => ready

(jvm-load-jar-sync (expand-file-name "target/test-fixtures/calc.jar"))
;; => (t)

(jvm-call-sync "Calc" "addLong" (list 2 3))
;; => (t 5 nil)   -- a static call; result is (OK RESULT ERR)

(jvm-call-sync "Calc" "callWithArg" (list (lambda (n) (* n 10)) 4))
;; => (t 40 nil)  -- the lambda is auto-wrapped as a real JVM object (a
;;    LispCallable); Calc.callWithArg calls it from Java and waits for
;;    the result
```

`jvm-call`/`jvm-call-sync` (and `jvm-new`/`jvm-new-sync`, for
constructors) reflectively dispatch on a class name, method name, and a
list of Lisp argument values. Numbers, bignums, strings, `nil`/`t`,
wrapped JVM objects (returned from an earlier `jvm-call`/`jvm-new`), and
Lisp functions all marshal automatically — a Lisp function argument
becomes a real async `LispCallable` the JVM side can call, and a JVM
object the JVM passes back into a Lisp callback arrives as a wrapped
Lisp value, not a raw handle. Errors on either side propagate to the
other as a real error, not a silent failure. Full design:
[`docs/superpowers/specs/2026-09-01-jvm-lisp-interop-design.md`](docs/superpowers/specs/2026-09-01-jvm-lisp-interop-design.md).

## Clojure & nREPL

Evaluate Clojure code, and run a real nREPL server other processes can
connect to — both built entirely on the `jvm-call`/`jvm-new` primitives
above; no new Rust code, no new wire messages. Full design:
[`docs/superpowers/specs/2026-09-02-clojure-nrepl-design.md`](docs/superpowers/specs/2026-09-02-clojure-nrepl-design.md).

**1. Fetch the Clojure + nREPL jars** (not bundled — same policy as the
JDK itself; see [Prerequisites](#prerequisites)):

```sh
./test/fetch-clojure-jars.sh
```

This downloads `clojure-1.12.0.jar`, `spec.alpha-0.5.238.jar`,
`core.specs.alpha-0.4.74.jar` (Maven Central), and `nrepl-1.7.0.jar`
(**Clojars**, not Maven Central — nREPL isn't published there) into
`target/clojure-jars/`.

**2. Launch Emacs, same as the Quickstart above:**

```sh
cargo build
EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" emacs -Q -L elisp
```

**3. In `*scratch*`, evaluate one form at a time:**

```elisp
(require 'emacs-jni)

(jvm-start-sync)
;; => ready

(dolist (jar (directory-files (getenv "EMACS_JNI_CLOJURE_JARS_DIR") t "\\.jar\\'"))
  (jvm-load-jar-sync jar))

(jvm-clojure-eval-string "(+ 40 (* 2 1))")
;; => (t 42 nil)   -- (OK RESULT ERR), same shape as `jvm-call-sync'

(setq my-nrepl (jvm-nrepl-start 7888))
;; => (t (7888 . #<user-ptr ...>) nil)   -- (OK (ACTUAL-PORT . SERVER) ERR)
```

**4. Connect a real nREPL client to `localhost:7888`** — any
bencode-speaking client works (`lein repl :connect localhost:7888`, an
editor's nREPL client, etc.). Verified with `nc` as a real separate OS
process:

```sh
code='(+ 100 23)'
len=${#code}
printf 'd2:op4:eval4:code%d:%s2:id1:1e' "$len" "$code" | nc 127.0.0.1 7888
```

This returns a real bencode response containing `5:value3:123` and
`6:statusl4:donee` — a genuine nREPL eval round trip, not a canned reply.

**5. Stop the server:**

```elisp
(jvm-nrepl-stop (cdr (nth 1 my-nrepl)))
;; => (t nil nil)
```

Two things worth knowing before relying on this:

- `*ns*` set by one `jvm-clojure-eval-string` call does **not** persist
  to the next one — each call may dispatch to a different underlying JVM
  thread (Phase 2's per-call model). Fold `(in-ns ...)` into the same
  eval string when it matters. This does not affect the nREPL server
  itself, which manages its own per-session namespace state correctly.
- Non-scalar results (keywords, vectors, maps, lazy seqs) come back as
  an opaque wrapped JVM object, same limitation as any other non-scalar
  `jvm-call` result — call `.toString()` or `pr-str` via another
  `jvm-call` for a printable form.

## API reference

All functions are async by default (return immediately; the JVM worker
thread does the work on a separate OS thread and reports back). Each has a
`-sync` variant that blocks the calling (Lisp) thread until the operation
finishes or a `timeout` (default 30s, first optional/keyword arg on each
`-sync` function) elapses, signaling `jvm-timeout` on expiry.

| Function | Signature | Notes |
|---|---|---|
| `jvm-status` | `()` | Instant, synchronous read of the current status symbol. No round trip to the worker thread. |
| `jvm-start` / `jvm-start-sync` | `(&optional classpath options timeout)` | Starts a fresh JVM, or resumes one that's `paused`. `classpath`/`options` default to `jvm-classpath`/`jvm-options` and are **only honored on a genuinely fresh start** (from `not-started`/`stopped`/`start-failed`/`stop-failed`/`crashed`) — resuming from `paused` reuses the running JVM's original options. |
| `jvm-pause` / `jvm-pause-sync` | `(&optional timeout)` | Detaches the worker thread from the JVM but keeps the JVM alive and the worker thread running, ready to resume. Legal only from `ready`. |
| `jvm-stop` / `jvm-stop-sync` | `(&optional timeout)` | Attempts a real `JNI_DestroyJavaVM` teardown. **Experimental** — see below. Legal from `ready` or `paused`. |
| `jvm-load-jar` / `jvm-load-jar-sync` | `(path &optional callback-or-timeout)` | Adds `path` to the one shared classloader (reflectively, via `URLClassLoader#addURL`). Every loaded jar can see every other loaded jar. Legal only when `ready`; does not change `jvm-status`. Async form takes an optional `(callback ok err)`; sync form returns `(ok . err)`. |

Config variables (`defvar`, not `defcustom` — set them with `setq` before
your first `jvm-start`):

- `jvm-classpath` — list of strings, additional classpath entries.
- `jvm-options` — list of strings, JVM flags (e.g. `"-Xmx512m"`). Defaults
  to `("-Xrs")`, which stops the JVM from installing its own
  SIGINT/SIGTERM/SIGHUP/SIGQUIT handlers — without it, since the JVM shares
  Emacs's process, `C-g` can terminate Emacs outright rather than just
  interrupting it. Keep `"-Xrs"` in the list if you override it, unless you
  specifically want those handlers installed (see `HANDOFF.md`).

Observability:

- `jvm-status-hook` — run with the new status symbol on every transition
  (including the transient `starting`/`pausing`/`stopping` states, set
  synchronously the moment you call the async function, before the worker
  thread has actually done anything).
- `*jvm-log*` buffer — every failure-carrying status message and every
  worker-thread `log` message, appended as plain text lines.

Status values: `not-started`, `starting`, `ready`, `pausing`, `paused`,
`stopping`, `stopped`, `start-failed`, `stop-failed`, `crashed`.

## Known limitations

These are accepted, documented tradeoffs for this phase, not bugs:

- **`jvm-stop` is best-effort.** It relies on `JNI_DestroyJavaVM`, which
  some JVM builds handle unreliably. Both `stopped` and `stop-failed` count
  as "the operation completed" rather than hanging — `jvm-start` after a
  `jvm-stop` is not guaranteed to work.
- **No per-jar unload.** All jars share one classloader; there's no way to
  remove a single jar without pausing/stopping the whole JVM.
- **One JVM per Emacs process.** A JNI-level constraint, not a design
  choice — you can't run two independent embedded JVMs in one Emacs.
- **Restart-after-crash/stop doesn't retain a real handle.** `jni` 0.22.4
  internally caches the `JavaVM` pointer in a process-global `OnceLock`, so
  a fresh `jvm-start` after a crash or stop transparently reuses the same
  underlying VM rather than performing a genuine reattachment or
  recreation. See the comment above `create_jvm` in `src/jvm_thread.rs`.

## Testing

```sh
cargo test --lib                          # pure-Rust unit tests, no Emacs/JVM
./test/run-ert-tests.sh                   # full integration suite, real embedded JVM
EMACS_JNI_TEST_FEATURES=test-support ./test/run-ert-tests.sh   # + crash-recovery test
```

Each `test/*.el` file runs in its own `emacs --batch` process (the embedded
JVM is a per-process singleton, so test files can't safely share one).

The 5 Clojure/nREPL tests in `test/jvm-clojure-test.el` are gated behind
`EMACS_JNI_CLOJURE_JARS_DIR` (see [Clojure & nREPL](#clojure--nrepl)
above) — they skip cleanly, not fail, when it's unset, so the rest of
the suite stays runnable with no network access:

```sh
./test/fetch-clojure-jars.sh
EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" EMACS_JNI_TEST_FEATURES=test-support ./test/run-ert-tests.sh
```

## Roadmap

- **Phase 1 (done):** JVM lifecycle + jar loading.
- **Phase 2 (done):** Bidirectional JVM ↔ Lisp interop — wrap a Lisp value
  as a JVM object, wrap a JVM object as a Lisp value, JVM calls a Lisp
  function, Lisp calls a JVM method. See [Interop example](#interop-example)
  above.
- **Phase 3 (done):** Clojure evaluation, a real nREPL server other
  processes can connect to. See [Clojure & nREPL](#clojure--nrepl)
  above.

See [`HANDOFF.md`](HANDOFF.md) for the current state in more detail and
what a next session should do.
