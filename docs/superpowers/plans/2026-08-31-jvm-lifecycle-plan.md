# JVM Lifecycle and Jar Loading Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Embed a JVM in an Emacs 28+ dynamic module (Linux), with lifecycle
control (`jvm-start`/`jvm-pause`/`jvm-stop`/`jvm-status`) and jar loading,
laying the foundation for Phase 2 (object interop) and Phase 3 (Clojure).

**Architecture:** A single dedicated OS worker thread owns the JVM for its
entire lifetime (from the first `jvm-start` until a successful `jvm-stop`).
Lisp-thread `#[defun]`s send commands to it over a Rust `mpsc` channel and
return immediately; the worker thread reports status/log/completion events
back to the Lisp thread by writing newline-terminated S-expressions through
`Env::open_channel`, which Emacs delivers to a pipe-process filter on the
main thread.

**Tech Stack:** Rust (`emacs` crate 0.21+ with `emacs-28` feature for the
dynamic module and `open_channel`; `jni` crate 0.22.4 with the `invocation`
feature for JVM embedding), Emacs Lisp, ERT for integration tests, `javac`
(JDK 26, already pinned via `mise.toml`) for a test fixture jar.

**Spec:** `docs/superpowers/specs/2026-08-31-jvm-lifecycle-design.md`

## Global Constraints

- Target: Emacs 28+, Linux only (per spec's Target Platform section).
- Crate: `jni` (jni-rs), not j4rs (per spec's Crate Choices section).
- Single Cargo crate, not a workspace, named `emacs-jni` (package) /
  `emacs_jni` (lib name) — used consistently in every file path, `mod`
  declaration, and doc comment below. Do not introduce any other crate name.
- `jvm-classpath` and `jvm-options` are plain `defvar`, never `defcustom`.
- Wire protocol: newline-terminated, hand-written S-expressions, no
  serde/json dependency. All `(status ...)` messages use the fixed 3-element
  shape `(status STATE MSG-OR-NIL)` (a refinement of the spec's wire-format
  section, made during planning for parser simplicity — the spec shows
  2-element `(status ready)` and 3-element `(status start-failed "msg")`
  variants; this plan standardizes on always-3-element for every `status`
  message, e.g. `(status ready nil)`).
- One persistent worker thread owns the JVM for its whole lifetime (spawned
  on the first `jvm-start`, exits only after a successful `jvm-stop` or an
  unrecoverable panic) — see "Design note" below for why this differs from
  the spec's literal wording.
- No per-jar unload in Phase 1 (shared classloader, per spec's Jar Loading
  section).

### Design note: single persistent worker thread (refines the spec)

The spec's architecture section describes `jvm-pause` as "the JVM thread
finishes its command loop and exits... `jvm-start` after a pause spawns a
*new* thread that calls `AttachCurrentThread` on the existing `JavaVM`."
While verifying the exact jni-rs 0.22.4 API against a real compiling probe
during planning, this turned out to need correction:

- jni-rs 0.22.4's `JavaVM::attach_current_thread` takes a callback
  (`FnOnce(&mut Env) -> Result<T, E>`) rather than returning a `AttachGuard`
  you hold across arbitrary calls. `JavaVM` itself is `#[derive(Clone)]`
  plus `unsafe impl Send + Sync` around a raw pointer, and `detach_current_thread(&self) -> Result<()>` /
  `unsafe fn destroy(&self) -> Result<()>` are both plain `&self` methods —
  none of this requires the JVM to be re-created or re-attached from a new
  OS thread.
- Modern JNI (JDK 1.2+) allows *any* thread to call `DestroyJavaVM`, not
  just the creating thread (verified via web search this session) — so
  there's no correctness reason to keep spawning new threads either.

Given this, the simpler and equally-correct design is: **one worker OS
thread for the JVM's entire lifetime.** `jvm-pause` calls
`vm.detach_current_thread()` explicitly but the thread keeps running,
blocked on `receiver.recv()`. `jvm-start` after a pause and `jvm-stop`
while paused both just send another command to the *same* already-open
channel — there is no "no worker thread to route the stop through while
paused" problem to solve. `Cmd::Stop`'s `vm.destroy()` runs inline on this
same worker thread; since that thread is already not Emacs's main thread, a
hang there doesn't block Emacs, so the spec's "throwaway teardown thread"
idea is dropped as unnecessary (this is the one deviation flagged for
review — see the plan's final report).

This changes the `Cmd` enum from `{Start, Pause, Stop, LoadJar}` to
`{Resume, Pause, Stop, LoadJar}`, with the very first "start" instead
passed as plain arguments to the thread-spawning function (never sent over
the channel, since there's no receiver yet to send it to). See Task 4.

---

## Task 1: Project scaffolding and smoke-test defun

**Files:**
- Create: `Cargo.toml`
- Create: `src/lib.rs`
- Create: `elisp/emacs-jni.el`
- Create: `test/run-ert-tests.sh`
- Create: `test/smoke-test.el`
- Create: `.gitignore` (if not already present from planning scratch work —
  check first; it may already contain `.probe/` and `/target` from a
  throwaway verification crate used during planning, which is fine, just
  add to it rather than overwrite)

**Interfaces:**
- Produces: a loadable dynamic module `target/debug/libemacs_jni.so`
  exporting one Lisp function `jvm-ping` (returns `t`), and
  `elisp/emacs-jni.el`, which subsequent tasks extend with real
  functionality without renaming anything defined here.

- [ ] **Step 1: Create the Cargo project files**

`Cargo.toml`:
```toml
[package]
name = "emacs-jni"
version = "0.1.0"
edition = "2021"

[lib]
name = "emacs_jni"
crate-type = ["cdylib"]

[dependencies]
emacs = { version = "0.21", features = ["emacs-28"] }
jni = { version = "0.22", features = ["invocation"] }

[features]
test-support = []
```

`src/lib.rs`:
```rust
use emacs::{defun, Env, Result, Value};

emacs::plugin_is_GPL_compatible!();

#[emacs::module]
fn init(env: &Env) -> Result<Value<'_>> {
    env.provide("emacs-jni")
}

#[defun(mod_in_name = false, name = "jvm-ping")]
fn jvm_ping() -> Result<bool> {
    Ok(true)
}
```

If `env.provide` doesn't exist on this version of the `emacs` crate (check
`cargo doc --open -p emacs` or the crate's `Env` docs if `cargo build`
fails on this line), replace the `init` body with:
```rust
fn init(env: &Env) -> Result<Value<'_>> {
    let feature = env.intern("emacs-jni")?;
    env.call("provide", (feature,))
}
```

- [ ] **Step 2: Build it**

Run: `cargo build`
Expected: succeeds, producing `target/debug/libemacs_jni.so`.

- [ ] **Step 3: Write the Elisp loader**

`elisp/emacs-jni.el`:
```elisp
;;; emacs-jni.el --- JVM embedding for Emacs -*- lexical-binding: t; -*-

(defvar emacs-jni--module-path
  (or (getenv "EMACS_JNI_MODULE_PATH")
      (expand-file-name "../target/debug/libemacs_jni.so"
                         (file-name-directory (or load-file-name buffer-file-name))))
  "Path to the built emacs-jni dynamic module (.so file).")

(unless (featurep 'emacs-jni)
  (module-load emacs-jni--module-path))

(provide 'emacs-jni)
;;; emacs-jni.el ends here
```

- [ ] **Step 4: Write the smoke-test ERT file**

`test/smoke-test.el`:
```elisp
;;; smoke-test.el --- Smoke test for module loading -*- lexical-binding: t; -*-

(require 'ert)
(require 'emacs-jni)

(ert-deftest emacs-jni-smoke-test/module-loads-and-responds ()
  (should (eq (jvm-ping) t)))

(provide 'smoke-test)
;;; smoke-test.el ends here
```

- [ ] **Step 5: Write the test runner script**

`test/run-ert-tests.sh`:
```bash
#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

CARGO_FEATURES="${EMACS_JNI_TEST_FEATURES:-}"
if [ -n "$CARGO_FEATURES" ]; then
    cargo build --features "$CARGO_FEATURES"
else
    cargo build
fi

export EMACS_JNI_MODULE_PATH="$(pwd)/target/debug/libemacs_jni.so"

TEST_FILES=("$@")
if [ ${#TEST_FILES[@]} -eq 0 ]; then
    TEST_FILES=(test/*.el)
fi

LOAD_ARGS=()
for f in "${TEST_FILES[@]}"; do
    LOAD_ARGS+=(-l "$f")
done

emacs -Q --batch -L elisp "${LOAD_ARGS[@]}" -f ert-run-tests-batch-and-exit
```

Run: `chmod +x test/run-ert-tests.sh`

- [ ] **Step 6: Run the smoke test**

Run: `./test/run-ert-tests.sh test/smoke-test.el`
Expected: `Ran 1 tests, 1 results as expected` (or ERT's equivalent success
summary), exit code 0.

- [ ] **Step 7: Set up .gitignore and commit**

Ensure `.gitignore` contains at least:
```
/target
.probe/
```
(the `.probe/` entry is harmless if there's no such directory; it guards
against reintroducing throwaway verification scratch work untracked).

```bash
git add Cargo.toml Cargo.lock src/lib.rs elisp/emacs-jni.el test/run-ert-tests.sh test/smoke-test.el .gitignore
git commit -m "Scaffold emacs-jni dynamic module with smoke-test defun"
```

---

## Task 2: Status state machine (pure Rust)

**Files:**
- Create: `src/status.rs`
- Modify: `src/lib.rs` (add `mod status;`)

**Interfaces:**
- Produces: `pub enum Status { NotStarted, Starting, Ready, Pausing, Paused,
  Stopping, Stopped, StartFailed, StopFailed, Crashed }` with
  `Status::as_tag(self) -> &'static str`; `pub struct StatusCell` with
  `get(&self) -> Status`, `begin_start(&self) -> Result<(), Status>`,
  `begin_pause(&self) -> Result<(), Status>`, `begin_stop(&self) -> Result<(), Status>`,
  `finish(&self, to: Status)`; `pub static STATUS: StatusCell`. Later tasks
  (4, 5, 6, 8) call `STATUS.begin_start()` / `begin_pause()` / `begin_stop()`
  from `#[defun]`s and `STATUS.finish(...)` from the worker thread.

- [ ] **Step 1: Write `src/status.rs` with unit tests**

```rust
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    NotStarted = 0,
    Starting = 1,
    Ready = 2,
    Pausing = 3,
    Paused = 4,
    Stopping = 5,
    Stopped = 6,
    StartFailed = 7,
    StopFailed = 8,
    Crashed = 9,
}

impl Status {
    fn from_u8(v: u8) -> Status {
        match v {
            0 => Status::NotStarted,
            1 => Status::Starting,
            2 => Status::Ready,
            3 => Status::Pausing,
            4 => Status::Paused,
            5 => Status::Stopping,
            6 => Status::Stopped,
            7 => Status::StartFailed,
            8 => Status::StopFailed,
            9 => Status::Crashed,
            _ => unreachable!("invalid Status discriminant"),
        }
    }

    pub fn as_tag(self) -> &'static str {
        match self {
            Status::NotStarted => "not-started",
            Status::Starting => "starting",
            Status::Ready => "ready",
            Status::Pausing => "pausing",
            Status::Paused => "paused",
            Status::Stopping => "stopping",
            Status::Stopped => "stopped",
            Status::StartFailed => "start-failed",
            Status::StopFailed => "stop-failed",
            Status::Crashed => "crashed",
        }
    }
}

pub struct StatusCell(AtomicU8);

impl StatusCell {
    pub const fn new() -> Self {
        StatusCell(AtomicU8::new(Status::NotStarted as u8))
    }

    pub fn get(&self) -> Status {
        Status::from_u8(self.0.load(Ordering::SeqCst))
    }

    fn set(&self, s: Status) {
        self.0.store(s as u8, Ordering::SeqCst);
    }

    fn try_transition(&self, allowed_from: &[Status], to: Status) -> Result<(), Status> {
        let current = self.get();
        if allowed_from.contains(&current) {
            self.set(to);
            Ok(())
        } else {
            Err(current)
        }
    }

    /// Legal from not-started, paused, stopped, start-failed, stop-failed,
    /// or crashed. Covers both a first-time start and a resume-from-pause;
    /// the caller (see worker.rs in Task 4) decides which by checking
    /// whether a worker thread is already running, independent of this
    /// state check.
    pub fn begin_start(&self) -> Result<(), Status> {
        self.try_transition(
            &[
                Status::NotStarted,
                Status::Paused,
                Status::Stopped,
                Status::StartFailed,
                Status::StopFailed,
                Status::Crashed,
            ],
            Status::Starting,
        )
    }

    pub fn begin_pause(&self) -> Result<(), Status> {
        self.try_transition(&[Status::Ready], Status::Pausing)
    }

    pub fn begin_stop(&self) -> Result<(), Status> {
        self.try_transition(&[Status::Ready, Status::Paused], Status::Stopping)
    }

    pub fn finish(&self, to: Status) {
        self.set(to);
    }
}

pub static STATUS: StatusCell = StatusCell::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_from_not_started_succeeds() {
        let cell = StatusCell::new();
        assert_eq!(cell.begin_start(), Ok(()));
        assert_eq!(cell.get(), Status::Starting);
    }

    #[test]
    fn start_from_paused_succeeds() {
        let cell = StatusCell::new();
        cell.finish(Status::Paused);
        assert_eq!(cell.begin_start(), Ok(()));
        assert_eq!(cell.get(), Status::Starting);
    }

    #[test]
    fn start_from_ready_fails() {
        let cell = StatusCell::new();
        cell.finish(Status::Ready);
        assert_eq!(cell.begin_start(), Err(Status::Ready));
        assert_eq!(cell.get(), Status::Ready);
    }

    #[test]
    fn pause_from_ready_succeeds() {
        let cell = StatusCell::new();
        cell.finish(Status::Ready);
        assert_eq!(cell.begin_pause(), Ok(()));
        assert_eq!(cell.get(), Status::Pausing);
    }

    #[test]
    fn pause_from_not_started_fails() {
        let cell = StatusCell::new();
        assert_eq!(cell.begin_pause(), Err(Status::NotStarted));
    }

    #[test]
    fn stop_from_ready_succeeds() {
        let cell = StatusCell::new();
        cell.finish(Status::Ready);
        assert_eq!(cell.begin_stop(), Ok(()));
        assert_eq!(cell.get(), Status::Stopping);
    }

    #[test]
    fn stop_from_paused_succeeds() {
        let cell = StatusCell::new();
        cell.finish(Status::Paused);
        assert_eq!(cell.begin_stop(), Ok(()));
        assert_eq!(cell.get(), Status::Stopping);
    }

    #[test]
    fn stop_from_not_started_fails() {
        let cell = StatusCell::new();
        assert_eq!(cell.begin_stop(), Err(Status::NotStarted));
    }

    #[test]
    fn as_tag_matches_spec_names() {
        assert_eq!(Status::NotStarted.as_tag(), "not-started");
        assert_eq!(Status::StartFailed.as_tag(), "start-failed");
        assert_eq!(Status::StopFailed.as_tag(), "stop-failed");
        assert_eq!(Status::Crashed.as_tag(), "crashed");
    }
}
```

- [ ] **Step 2: Wire up the module**

In `src/lib.rs`, add near the top (after the `use` line):
```rust
mod status;
```

- [ ] **Step 3: Run the unit tests**

Run: `cargo test --lib status::`
Expected: all `status::tests::*` tests pass.

- [ ] **Step 4: Run the ERT smoke test again to confirm nothing broke**

Run: `./test/run-ert-tests.sh test/smoke-test.el`
Expected: still passes (this task adds no new Lisp-visible behavior).

- [ ] **Step 5: Commit**

```bash
git add src/status.rs src/lib.rs
git commit -m "Add Status state machine with transition gating"
```

---

## Task 3: Wire protocol encoding (pure Rust)

**Files:**
- Create: `src/protocol.rs`
- Modify: `src/lib.rs` (add `mod protocol;`)

**Interfaces:**
- Consumes: `crate::status::Status` (Task 2).
- Produces: `pub fn status_message(status: Status, message: Option<&str>) -> String`,
  `pub fn load_jar_done_message(id: u64, ok: bool, err: Option<&str>) -> String`,
  `pub fn log_message(level: &str, message: &str) -> String`. Used by
  `src/channel.rs` and `src/jvm_thread.rs` starting in Task 4.

- [ ] **Step 1: Write `src/protocol.rs` with unit tests**

```rust
use crate::status::Status;

pub fn status_message(status: Status, message: Option<&str>) -> String {
    let msg_sexp = match message {
        Some(m) => quote(m),
        None => "nil".to_string(),
    };
    format!("(status {} {})\n", status.as_tag(), msg_sexp)
}

pub fn load_jar_done_message(id: u64, ok: bool, err: Option<&str>) -> String {
    let ok_sym = if ok { "t" } else { "nil" };
    let err_sexp = match err {
        Some(e) => quote(e),
        None => "nil".to_string(),
    };
    format!("(load-jar-done {} {} {})\n", id, ok_sym, err_sexp)
}

pub fn log_message(level: &str, message: &str) -> String {
    format!("(log {} {})\n", level, quote(message))
}

/// Renders a Rust string as an Emacs Lisp string literal: wraps in double
/// quotes and escapes backslashes and double quotes. This only needs to
/// cover what our own messages ever contain (plain error/log text), not
/// full Lisp reader syntax.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_simple_status() {
        assert_eq!(status_message(Status::Ready, None), "(status ready nil)\n");
    }

    #[test]
    fn formats_status_with_message() {
        assert_eq!(
            status_message(Status::StartFailed, Some("boom")),
            "(status start-failed \"boom\")\n"
        );
    }

    #[test]
    fn escapes_quotes_and_backslashes_in_message() {
        assert_eq!(
            status_message(Status::Crashed, Some("bad \"quote\" and \\slash")),
            "(status crashed \"bad \\\"quote\\\" and \\\\slash\")\n"
        );
    }

    #[test]
    fn formats_load_jar_done_success() {
        assert_eq!(load_jar_done_message(3, true, None), "(load-jar-done 3 t nil)\n");
    }

    #[test]
    fn formats_load_jar_done_failure() {
        assert_eq!(
            load_jar_done_message(3, false, Some("not found")),
            "(load-jar-done 3 nil \"not found\")\n"
        );
    }

    #[test]
    fn formats_log_message() {
        assert_eq!(log_message("info", "hello"), "(log info \"hello\")\n");
    }
}
```

- [ ] **Step 2: Wire up the module**

In `src/lib.rs`, add:
```rust
mod protocol;
```

- [ ] **Step 3: Run the unit tests**

Run: `cargo test --lib protocol::`
Expected: all `protocol::tests::*` tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/protocol.rs src/lib.rs
git commit -m "Add wire protocol S-expression encoding"
```

---

## Task 4: Channel plumbing, worker thread, and jvm-start end-to-end

This is the first task that creates a real JVM. It introduces five new
files and is the largest task in this plan; do not split it further, since
none of these pieces is independently testable without the others (the
channel needs a real command to prove it end-to-end, and `jvm-start` needs
the channel to report its result).

**Files:**
- Create: `src/channel.rs`
- Create: `src/commands.rs`
- Create: `src/worker.rs`
- Create: `src/jvm_thread.rs`
- Modify: `src/lib.rs` (add `mod` declarations, `jvm--init-channel` and
  `jvm--start` defuns)
- Modify: `elisp/emacs-jni.el` (pipe process, filter, `jvm-classpath`,
  `jvm-options`, `jvm--status`, `jvm-status-hook`, `jvm-status`, `jvm-start`,
  `jvm-start-sync`)
- Create: `test/jvm-start-test.el`

**Interfaces:**
- Consumes: `crate::status::{Status, STATUS}` (Task 2),
  `crate::protocol::status_message` (Task 3).
- Produces: `channel::init(env: &Env, process: Value) -> Result<()>`,
  `channel::send(message: &str)`; `commands::Cmd` enum with `Resume`,
  `Pause`, `Stop`, `LoadJar { id: u64, path: String }` variants (only
  `Resume` is handled meaningfully by the end of this task — the others are
  added to the `match` in Tasks 5-7, so the `match` in `jvm_thread::run`
  must be non-exhaustive-safe from the start, e.g. via a catch-all arm that
  Tasks 5-7 replace); `worker::start_or_resume(classpath: Vec<String>, options: Vec<String>)`,
  `worker::clear_sender()`, `worker::next_request_id() -> u64`,
  `worker::send_command(cmd: Cmd) -> Result<(), ()>`; `jvm_thread::run(receiver, classpath, options)`.
  Elisp: `jvm-classpath`, `jvm-options` (defvar), `jvm--status` (defvar),
  `jvm-status-hook` (defvar), `jvm-status` (defun), `jvm-start` (defun),
  `jvm-start-sync` (defun). Later tasks extend `elisp/emacs-jni.el`'s filter
  dispatch and the `Cmd`/`match` without renaming any of these.

- [ ] **Step 1: Write `src/channel.rs`**

```rust
use std::io::Write;
use std::sync::Mutex;

use emacs::{Env, Result, Value};

static WRITER: Mutex<Option<Box<dyn Write + Send>>> = Mutex::new(None);

/// Opens the channel against the given pipe-process Lisp value and stores
/// the writer globally, replacing any previous one.
pub fn init(env: &Env, process: Value) -> Result<()> {
    let writer = env.open_channel(process)?;
    *WRITER.lock().unwrap() = Some(Box::new(writer));
    Ok(())
}

/// Writes a pre-formatted message (as produced by the `protocol` module) to
/// the channel. Silently drops the message if the channel hasn't been
/// initialized or the write fails — the worker thread has no way to
/// recover from a broken channel; the Elisp-side `-sync` wrappers' timeouts
/// are what catch a message that never arrives.
pub fn send(message: &str) {
    if let Some(writer) = WRITER.lock().unwrap().as_mut() {
        let _ = writer.write_all(message.as_bytes());
    }
}
```

- [ ] **Step 2: Write `src/commands.rs`**

```rust
#[derive(Debug)]
pub enum Cmd {
    /// Sent to an already-running worker thread to resume from `paused`.
    /// A first-time start never goes through this enum at all — see
    /// `worker::start_or_resume`, which spawns a fresh thread with
    /// classpath/options as plain arguments instead.
    Resume,
    Pause,
    Stop,
    LoadJar { id: u64, path: String },
}
```

- [ ] **Step 3: Write `src/worker.rs`**

```rust
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::thread;

use crate::commands::Cmd;
use crate::jvm_thread;

static SENDER: Mutex<Option<Sender<Cmd>>> = Mutex::new(None);
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_request_id() -> u64 {
    NEXT_REQUEST_ID.fetch_add(1, Ordering::SeqCst)
}

/// Resumes the existing worker thread if one is alive (paused case), or
/// spawns a brand new one (first start, or restart after stop/crash).
pub fn start_or_resume(classpath: Vec<String>, options: Vec<String>) {
    let mut guard = SENDER.lock().unwrap();
    if let Some(sender) = guard.as_ref() {
        let _ = sender.send(Cmd::Resume);
    } else {
        let (tx, rx) = mpsc::channel();
        *guard = Some(tx);
        thread::spawn(move || jvm_thread::run(rx, classpath, options));
    }
}

/// Sends a non-Resume command to the live worker thread. Returns `Err(())`
/// if there is no worker thread currently running (shouldn't happen if
/// callers check `Status` first, but kept fallible rather than panicking).
pub fn send_command(cmd: Cmd) -> Result<(), ()> {
    let guard = SENDER.lock().unwrap();
    match guard.as_ref() {
        Some(sender) => sender.send(cmd).map_err(|_| ()),
        None => Err(()),
    }
}

/// Called by the worker thread itself right before it exits for good
/// (successful stop, or an unrecoverable panic), so the next `jvm-start`
/// knows to spawn a fresh thread instead of trying to resume a dead one.
pub fn clear_sender() {
    *SENDER.lock().unwrap() = None;
}
```

- [ ] **Step 4: Write `src/jvm_thread.rs`**

```rust
use std::sync::mpsc::Receiver;

use jni::vm::{InitArgsBuilder, JavaVM};
use jni::JNIVersion;

use crate::commands::Cmd;
use crate::protocol;
use crate::status::{Status, STATUS};
use crate::{channel, worker};

/// Entry point for the JVM worker thread. Runs for the JVM's entire
/// lifetime: creates the JVM once, then loops handling commands until a
/// successful `Cmd::Stop` (or, from Task 8 on, an unrecoverable panic).
pub fn run(receiver: Receiver<Cmd>, classpath: Vec<String>, options: Vec<String>) {
    let vm = match create_jvm(&classpath, &options) {
        Ok(vm) => {
            STATUS.finish(Status::Ready);
            channel::send(&protocol::status_message(Status::Ready, None));
            vm
        }
        Err(e) => {
            STATUS.finish(Status::StartFailed);
            channel::send(&protocol::status_message(Status::StartFailed, Some(&e)));
            worker::clear_sender();
            return;
        }
    };

    for cmd in receiver {
        match cmd {
            Cmd::Resume => handle_resume(&vm),
            // Handled starting in Task 5, 6, 7 respectively. This
            // catch-all keeps the match exhaustive in the meantime; each
            // task that adds real handling replaces the corresponding
            // line here (see that task's Step for the exact diff).
            Cmd::Pause | Cmd::Stop | Cmd::LoadJar { .. } => {}
        }
    }
}

fn create_jvm(classpath: &[String], options: &[String]) -> Result<JavaVM, String> {
    let mut builder = InitArgsBuilder::new().version(JNIVersion::V1_8);
    if !classpath.is_empty() {
        builder = builder.option(format!("-Djava.class.path={}", classpath.join(":")));
    }
    for opt in options {
        builder = builder.option(opt.clone());
    }
    let init_args = builder.build().map_err(|e| e.to_string())?;
    JavaVM::new(init_args).map_err(|e| e.to_string())
}

fn handle_resume(vm: &JavaVM) {
    // Cheap: attach_current_thread is a fast thread-local check if the
    // thread is already attached, and a real (re-)attach if `Cmd::Pause`
    // (Task 5) had explicitly detached it.
    let result: jni::errors::Result<()> = vm.attach_current_thread(|_env| Ok(()));
    match result {
        Ok(()) => {
            STATUS.finish(Status::Ready);
            channel::send(&protocol::status_message(Status::Ready, None));
        }
        Err(e) => {
            STATUS.finish(Status::StartFailed);
            channel::send(&protocol::status_message(Status::StartFailed, Some(&e.to_string())));
        }
    }
}
```

Note: `InitArgsBuilder::option` takes `impl Into<Cow<str>>` in jni-rs
0.22.4 (verified against the real crate source during planning), so passing
an owned `String` (via `format!` or `.clone()`) works directly without
needing `.as_str()` juggling.

- [ ] **Step 5: Wire up the module and add the two defuns**

In `src/lib.rs`, add near the top:
```rust
mod channel;
mod commands;
mod jvm_thread;
mod worker;
```

Add after `jvm_ping`:
```rust
use crate::status::{Status, STATUS};

emacs::define_errors! {
    jvm_illegal_transition "JVM operation not allowed in the current state"
}

#[defun(mod_in_name = false, name = "jvm--init-channel")]
fn jvm_init_channel(env: &Env, process: Value) -> Result<()> {
    channel::init(env, process)
}

#[defun(mod_in_name = false, name = "jvm--start")]
fn jvm_start(env: &Env, classpath: Vec<String>, options: Vec<String>) -> Result<()> {
    match STATUS.begin_start() {
        Ok(()) => {
            worker::start_or_resume(classpath, options);
            Ok(())
        }
        Err(current) => env.signal(jvm_illegal_transition, (current.as_tag(),)),
    }
}
```

- [ ] **Step 6: Run `cargo build` and fix any compile errors**

Run: `cargo build`
Expected: succeeds. If `InitArgsBuilder`/`JavaVM`/`JNIVersion` import paths
differ from what's shown (jni-rs has moved types between `jni::` and
`jni::vm::` across versions before), adjust the `use` statements in
`src/jvm_thread.rs` to match whatever `cargo build`'s error output shows —
do not guess silently, use the compiler's suggested paths.

- [ ] **Step 7: Extend `elisp/emacs-jni.el`**

Add after the `(provide 'emacs-jni)` line is removed temporarily (move it
to the end of the file, after all definitions — Elisp files should
`provide` last):

```elisp
(defvar jvm-classpath nil
  "List of paths added to the JVM's classpath when it starts.
Only honored on a genuinely fresh `jvm-start' (from `not-started',
`stopped', `start-failed', `stop-failed', or `crashed'). Resuming from
`paused' reuses the already-running JVM and ignores this variable, since
JVM options cannot change after the JVM is created.")

(defvar jvm-options nil
  "List of JVM flag strings (e.g. \"-Xmx512m\") passed at `jvm-start' time.
Same first-start-only caveat as `jvm-classpath'.")

(defvar jvm--status 'not-started
  "Current JVM lifecycle status. See `jvm-status'.")

(defvar jvm-status-hook nil
  "Hook run with the new status symbol on every JVM status transition.")

(defvar jvm--pipe-process nil
  "The pipe process used to receive messages from the JVM worker thread.")

(defvar jvm--filter-buffer ""
  "Accumulates partial output from `jvm--pipe-process' between newlines.")

(defun jvm--dispatch-message (sexp)
  "Handle one parsed message SEXP from the JVM worker thread."
  (pcase sexp
    (`(status ,state ,_msg)
     (setq jvm--status state)
     (run-hook-with-args 'jvm-status-hook state))
    (_ (message "emacs-jni: unrecognized channel message: %S" sexp))))

(defun jvm--process-filter (_proc data)
  "Accumulate DATA and dispatch each complete newline-terminated S-expression."
  (setq jvm--filter-buffer (concat jvm--filter-buffer data))
  (let (newline-pos)
    (while (setq newline-pos (string-search "\n" jvm--filter-buffer))
      (let ((line (substring jvm--filter-buffer 0 newline-pos)))
        (setq jvm--filter-buffer (substring jvm--filter-buffer (1+ newline-pos)))
        (unless (string-empty-p line)
          (jvm--dispatch-message (read line)))))))

(defun jvm--ensure-pipe-process ()
  "Create the pipe process and open the Rust-side channel, once."
  (unless (process-live-p jvm--pipe-process)
    (setq jvm--pipe-process
          (make-pipe-process
           :name "emacs-jni-channel"
           :filter #'jvm--process-filter
           :noquery t))
    (jvm--init-channel jvm--pipe-process)))

(defun jvm-status ()
  "Return the current JVM lifecycle status symbol."
  jvm--status)

(defun jvm-start (&optional classpath options)
  "Start (or resume) the JVM asynchronously.
CLASSPATH and OPTIONS default to `jvm-classpath' and `jvm-options'."
  (jvm--ensure-pipe-process)
  (setq jvm--status 'starting)
  (run-hook-with-args 'jvm-status-hook 'starting)
  (jvm--start (or classpath jvm-classpath) (or options jvm-options)))

(define-error 'jvm-timeout "Timed out waiting for the JVM")

(defun jvm-start-sync (&optional classpath options timeout)
  "Like `jvm-start', but blocks until the JVM is ready or TIMEOUT (default 30s) elapses."
  (jvm-start classpath options)
  (let ((deadline (+ (float-time) (or timeout 30))))
    (while (eq (jvm-status) 'starting)
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (signal 'jvm-timeout (list 'jvm-start-sync))))))
  (jvm-status))

(provide 'emacs-jni)
```

- [ ] **Step 8: Write `test/jvm-start-test.el`**

```elisp
;;; jvm-start-test.el --- Tests for jvm-start -*- lexical-binding: t; -*-

(require 'ert)
(require 'emacs-jni)

(ert-deftest emacs-jni-jvm-start-test/reaches-ready ()
  (should (eq (jvm-start-sync) 'ready)))

(provide 'jvm-start-test)
;;; jvm-start-test.el ends here
```

- [ ] **Step 9: Run it**

Run: `./test/run-ert-tests.sh test/jvm-start-test.el`
Expected: passes. This is the first test that creates a real JVM inside
the Emacs batch process; if it hangs, check that `libjvm.so` is
discoverable — jni-rs's `invocation` feature locates it via `JAVA_HOME` or
`java` on `PATH` at runtime (via the `java-locator` dependency), so running
the test with `JAVA_HOME` set to the JDK 26 install (`mise which java` or
equivalent) may be required in a shell that doesn't already have it
exported; if so, add `export JAVA_HOME=...` to `test/run-ert-tests.sh`
rather than requiring every invocation to set it manually.

- [ ] **Step 10: Commit**

```bash
git add src/channel.rs src/commands.rs src/worker.rs src/jvm_thread.rs src/lib.rs elisp/emacs-jni.el test/jvm-start-test.el
git commit -m "Add channel plumbing, worker thread, and jvm-start end-to-end"
```

---

## Task 5: jvm-pause and resume

**Files:**
- Modify: `src/jvm_thread.rs` (replace the `Cmd::Pause` catch-all arm with
  real handling)
- Modify: `src/lib.rs` (add `jvm--pause` defun)
- Modify: `elisp/emacs-jni.el` (add `jvm-pause`, `jvm-pause-sync`)
- Create: `test/jvm-pause-test.el`

**Interfaces:**
- Consumes: `worker::send_command` (Task 4).
- Produces: `jvm--pause` defun; Elisp `jvm-pause`, `jvm-pause-sync`.

- [ ] **Step 1: Replace the `Cmd::Pause` arm in `src/jvm_thread.rs`**

Change:
```rust
        match cmd {
            Cmd::Resume => handle_resume(&vm),
            // Handled starting in Task 5, 6, 7 respectively. This
            // catch-all keeps the match exhaustive in the meantime; each
            // task that adds real handling replaces the corresponding
            // line here (see that task's Step for the exact diff).
            Cmd::Pause | Cmd::Stop | Cmd::LoadJar { .. } => {}
        }
```
to:
```rust
        match cmd {
            Cmd::Resume => handle_resume(&vm),
            Cmd::Pause => handle_pause(&vm),
            // Handled starting in Task 6, 7 respectively.
            Cmd::Stop | Cmd::LoadJar { .. } => {}
        }
```

Add the handler function below `handle_resume`:
```rust
fn handle_pause(vm: &JavaVM) {
    match vm.detach_current_thread() {
        Ok(()) => {
            STATUS.finish(Status::Paused);
            channel::send(&protocol::status_message(Status::Paused, None));
        }
        Err(e) => {
            // Detach failing is unexpected; stay in `pausing` rather than
            // claiming success we can't back up, and surface it as a log
            // line rather than a status (there's no `pause-failed` state
            // in the spec's state machine, so pausing simply never
            // completes — the caller's `-sync` wrapper times out).
            channel::send(&protocol::log_message(
                "error",
                &format!("jvm-pause: detach_current_thread failed: {e}"),
            ));
        }
    }
}
```

- [ ] **Step 2: Add the `jvm--pause` defun in `src/lib.rs`**

```rust
#[defun(mod_in_name = false, name = "jvm--pause")]
fn jvm_pause(env: &Env) -> Result<()> {
    match STATUS.begin_pause() {
        Ok(()) => match worker::send_command(commands::Cmd::Pause) {
            Ok(()) => Ok(()),
            Err(()) => env.signal(jvm_illegal_transition, ("no running JVM worker thread",)),
        },
        Err(current) => env.signal(jvm_illegal_transition, (current.as_tag(),)),
    }
}
```

- [ ] **Step 3: Run `cargo build` and `cargo test`**

Run: `cargo build && cargo test --lib`
Expected: both succeed (no unit tests added this task; this just confirms
nothing broke).

- [ ] **Step 4: Extend `elisp/emacs-jni.el`**

Insert before `(provide 'emacs-jni)`:
```elisp
(defun jvm-pause ()
  "Pause the JVM asynchronously (detaches the worker thread; JVM state is retained)."
  (setq jvm--status 'pausing)
  (run-hook-with-args 'jvm-status-hook 'pausing)
  (jvm--pause))

(defun jvm-pause-sync (&optional timeout)
  "Like `jvm-pause', but blocks until paused or TIMEOUT (default 30s) elapses."
  (jvm-pause)
  (let ((deadline (+ (float-time) (or timeout 30))))
    (while (eq (jvm-status) 'pausing)
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (signal 'jvm-timeout (list 'jvm-pause-sync))))))
  (jvm-status))
```

- [ ] **Step 5: Write `test/jvm-pause-test.el`**

```elisp
;;; jvm-pause-test.el --- Tests for jvm-pause/resume -*- lexical-binding: t; -*-

(require 'ert)
(require 'emacs-jni)

(ert-deftest emacs-jni-jvm-pause-test/pause-then-resume ()
  (should (eq (jvm-start-sync) 'ready))
  (should (eq (jvm-pause-sync) 'paused))
  (should (eq (jvm-start-sync) 'ready)))

(provide 'jvm-pause-test)
;;; jvm-pause-test.el ends here
```

- [ ] **Step 6: Run it**

Run: `./test/run-ert-tests.sh test/jvm-pause-test.el`
Expected: passes.

- [ ] **Step 7: Commit**

```bash
git add src/jvm_thread.rs src/lib.rs elisp/emacs-jni.el test/jvm-pause-test.el
git commit -m "Add jvm-pause and resume-from-pause"
```

---

## Task 6: jvm-stop

**Files:**
- Modify: `src/jvm_thread.rs` (replace the `Cmd::Stop` catch-all arm)
- Modify: `src/lib.rs` (add `jvm--stop` defun)
- Modify: `elisp/emacs-jni.el` (add `jvm-stop`, `jvm-stop-sync`)
- Create: `test/jvm-stop-test.el`

**Interfaces:**
- Consumes: `worker::clear_sender` (Task 4).
- Produces: `jvm--stop` defun; Elisp `jvm-stop`, `jvm-stop-sync`.

- [ ] **Step 1: Replace the `Cmd::Stop` arm and change `run`'s control flow**

`Cmd::Stop` needs to end the `for cmd in receiver` loop (the thread exits
for good), which a plain `match` arm can't do on its own — change the loop
to break out based on the handler's return value:

Change:
```rust
    for cmd in receiver {
        match cmd {
            Cmd::Resume => handle_resume(&vm),
            Cmd::Pause => handle_pause(&vm),
            // Handled starting in Task 6, 7 respectively.
            Cmd::Stop | Cmd::LoadJar { .. } => {}
        }
    }
```
to:
```rust
    for cmd in receiver {
        match cmd {
            Cmd::Resume => handle_resume(&vm),
            Cmd::Pause => handle_pause(&vm),
            Cmd::Stop => {
                handle_stop(vm);
                break;
            }
            // Handled starting in Task 7.
            Cmd::LoadJar { .. } => {}
        }
    }
```

Add the handler (note it takes `vm` by value, since `destroy()` leaves it
in a state that must be dropped and nothing after this point needs it
again):
```rust
fn handle_stop(vm: JavaVM) {
    // Safety: `destroy` requires no Java methods on the calling thread's
    // stack (true here — we're between command handlers) and that the
    // JavaVM isn't used again afterward except to drop it, which is
    // exactly what follows.
    let result = unsafe { vm.destroy() };
    std::mem::drop(vm);
    match result {
        Ok(()) => {
            STATUS.finish(Status::Stopped);
            channel::send(&protocol::status_message(Status::Stopped, None));
        }
        Err(e) => {
            STATUS.finish(Status::StopFailed);
            channel::send(&protocol::status_message(Status::StopFailed, Some(&e.to_string())));
        }
    }
    worker::clear_sender();
}
```

- [ ] **Step 2: Add the `jvm--stop` defun in `src/lib.rs`**

```rust
#[defun(mod_in_name = false, name = "jvm--stop")]
fn jvm_stop(env: &Env) -> Result<()> {
    match STATUS.begin_stop() {
        Ok(()) => match worker::send_command(commands::Cmd::Stop) {
            Ok(()) => Ok(()),
            Err(()) => env.signal(jvm_illegal_transition, ("no running JVM worker thread",)),
        },
        Err(current) => env.signal(jvm_illegal_transition, (current.as_tag(),)),
    }
}
```

- [ ] **Step 3: Run `cargo build` and `cargo test`**

Run: `cargo build && cargo test --lib`
Expected: both succeed.

- [ ] **Step 4: Extend `elisp/emacs-jni.el`**

Insert before `(provide 'emacs-jni)`:
```elisp
(defun jvm-stop ()
  "Attempt to fully tear down the JVM asynchronously. Experimental: this
relies on JNI_DestroyJavaVM, which some JVM builds handle unreliably (see
the design spec's Risks section). `jvm-start' afterward is best-effort."
  (setq jvm--status 'stopping)
  (run-hook-with-args 'jvm-status-hook 'stopping)
  (jvm--stop))

(defun jvm-stop-sync (&optional timeout)
  "Like `jvm-stop', but blocks until stopped/stop-failed or TIMEOUT (default 30s)."
  (jvm-stop)
  (let ((deadline (+ (float-time) (or timeout 30))))
    (while (eq (jvm-status) 'stopping)
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (signal 'jvm-timeout (list 'jvm-stop-sync))))))
  (jvm-status))
```

- [ ] **Step 5: Write `test/jvm-stop-test.el`**

```elisp
;;; jvm-stop-test.el --- Tests for jvm-stop -*- lexical-binding: t; -*-

(require 'ert)
(require 'emacs-jni)

(ert-deftest emacs-jni-jvm-stop-test/stop-from-ready ()
  (should (eq (jvm-start-sync) 'ready))
  (should (memq (jvm-stop-sync) '(stopped stop-failed))))

(ert-deftest emacs-jni-jvm-stop-test/stop-from-paused ()
  (should (eq (jvm-start-sync) 'ready))
  (should (eq (jvm-pause-sync) 'paused))
  (should (memq (jvm-stop-sync) '(stopped stop-failed))))

(provide 'jvm-stop-test)
;;; jvm-stop-test.el ends here
```

Both tests accept either `stopped` or `stop-failed` as a pass, per the
spec's explicitly experimental/best-effort framing of `jvm-stop` — the
point of the test is that the operation *completes* (doesn't hang forever
past the timeout), not that teardown always succeeds.

- [ ] **Step 6: Run it**

Run: `./test/run-ert-tests.sh test/jvm-stop-test.el`
Expected: passes. If it times out instead (signals `jvm-timeout`), that's
a real finding to report, not silently work around — it would mean
`JavaVM::destroy()` hung on this system, which is the exact documented
risk from the spec; do not add retries or swallow the error, just note it.

- [ ] **Step 7: Commit**

```bash
git add src/jvm_thread.rs src/lib.rs elisp/emacs-jni.el test/jvm-stop-test.el
git commit -m "Add jvm-stop"
```

---

## Task 7: jvm-load-jar (shared classloader via reflection)

This task's reflective-`addURL` approach was verified against a real
compiling-and-running probe program on the actual JDK 26 installed on this
machine during planning (not just read from documentation) — every JNI
call sequence below is copied from that verified probe.

**Files:**
- Modify: `src/jvm_thread.rs` (replace the `Cmd::LoadJar` catch-all arm;
  add classloader state to the worker's owned data)
- Modify: `src/lib.rs` (add `jvm--load-jar` defun)
- Modify: `elisp/emacs-jni.el` (add `jvm-load-jar`, `jvm-load-jar-sync`,
  pending-callback table, extend the filter's dispatch)
- Create: `test/fixtures/Greeter.java`
- Modify: `test/run-ert-tests.sh` (build the fixture jar)
- Create: `test/jvm-load-jar-test.el`

**Interfaces:**
- Consumes: `protocol::load_jar_done_message` (Task 3).
- Produces: `jvm--load-jar` defun (returns `u64` request id); Elisp
  `jvm-load-jar`, `jvm-load-jar-sync`.

- [ ] **Step 1: Add classloader state and the `Cmd::LoadJar` handler in `src/jvm_thread.rs`**

The classloader is created lazily on the first `jvm-load-jar` call and
reused for every subsequent one, so it needs to live across loop
iterations. Change `run`'s body to track it:

```rust
pub fn run(receiver: Receiver<Cmd>, classpath: Vec<String>, options: Vec<String>) {
    let vm = match create_jvm(&classpath, &options) {
        Ok(vm) => {
            STATUS.finish(Status::Ready);
            channel::send(&protocol::status_message(Status::Ready, None));
            vm
        }
        Err(e) => {
            STATUS.finish(Status::StartFailed);
            channel::send(&protocol::status_message(Status::StartFailed, Some(&e)));
            worker::clear_sender();
            return;
        }
    };

    // Lazily created on the first LoadJar command, then reused so every
    // loaded jar shares one classloader (see spec: "Jar loading").
    let mut loader: Option<jni::objects::Global<jni::objects::JObject<'static>>> = None;

    for cmd in receiver {
        match cmd {
            Cmd::Resume => handle_resume(&vm),
            Cmd::Pause => handle_pause(&vm),
            Cmd::Stop => {
                handle_stop(vm);
                break;
            }
            Cmd::LoadJar { id, path } => handle_load_jar(&vm, &mut loader, id, path),
        }
    }
}
```

Add the handler and its helper below `handle_pause`:
```rust
fn handle_load_jar(
    vm: &JavaVM,
    loader: &mut Option<jni::objects::Global<jni::objects::JObject<'static>>>,
    id: u64,
    path: String,
) {
    let result: jni::errors::Result<()> = vm.attach_current_thread(|env| {
        if loader.is_none() {
            *loader = Some(create_shared_classloader(env)?);
        }
        let loader_local = env.new_local_ref(loader.as_ref().unwrap().as_obj())?;
        add_jar_url(env, &loader_local, &path)
    });

    match result {
        Ok(()) => channel::send(&protocol::load_jar_done_message(id, true, None)),
        Err(e) => channel::send(&protocol::load_jar_done_message(id, false, Some(&e.to_string()))),
    }
}

/// Creates a `URLClassLoader` with an empty URL list, parented on the
/// platform classloader (not the application classloader) so jars we load
/// don't inherit whatever's already on this module's own classpath.
fn create_shared_classloader<'local>(
    env: &mut jni::Env<'local>,
) -> jni::errors::Result<jni::objects::Global<jni::objects::JObject<'static>>> {
    let class_loader_class = env.find_class(jni::jni_str!("java/lang/ClassLoader"))?;
    let platform_loader = env
        .call_static_method(
            &class_loader_class,
            jni::jni_str!("getPlatformClassLoader"),
            jni::jni_sig!(() -> java.lang.ClassLoader),
            &[],
        )?
        .l()?;

    let url_class = env.find_class(jni::jni_str!("java/net/URL"))?;
    let empty_urls = env.new_object_array(0, &url_class, jni::objects::JObject::null())?;

    let loader_class = env.find_class(jni::jni_str!("java/net/URLClassLoader"))?;
    let loader = env.new_object(
        &loader_class,
        jni::jni_sig!((urls: [java.net.URL], parent: java.lang.ClassLoader) -> void),
        &[
            jni::objects::JValue::Object(&empty_urls),
            jni::objects::JValue::Object(&platform_loader),
        ],
    )?;

    env.new_global_ref(&loader)
}

/// Converts PATH to a `file:` URL and adds it to LOADER via the protected
/// `URLClassLoader#addURL(URL)` method, called reflectively (Java gives no
/// public API to extend an existing URLClassLoader's URL list).
fn add_jar_url(
    env: &mut jni::Env<'_>,
    loader: &jni::objects::JObject<'_>,
    path: &str,
) -> jni::errors::Result<()> {
    let path_str = env.new_string(path)?;
    let file_class = env.find_class(jni::jni_str!("java/io/File"))?;
    let file = env.new_object(
        &file_class,
        jni::jni_sig!((pathname: java.lang.String) -> void),
        &[jni::objects::JValue::Object(&path_str.into())],
    )?;
    let uri = env
        .call_method(&file, jni::jni_str!("toURI"), jni::jni_sig!(() -> java.net.URI), &[])?
        .l()?;
    let url = env
        .call_method(&uri, jni::jni_str!("toURL"), jni::jni_sig!(() -> java.net.URL), &[])?
        .l()?;

    let url_class = env.find_class(jni::jni_str!("java/net/URL"))?;
    let loader_class = env.find_class(jni::jni_str!("java/net/URLClassLoader"))?;
    let class_class = env.find_class(jni::jni_str!("java/lang/Class"))?;
    let param_types = env.new_object_array(1, &class_class, &url_class)?;

    let method_name = env.new_string("addURL")?;
    let method = env
        .call_method(
            &loader_class,
            jni::jni_str!("getDeclaredMethod"),
            jni::jni_sig!((name: java.lang.String, parameterTypes: [java.lang.Class]) -> java.lang.reflect.Method),
            &[
                jni::objects::JValue::Object(&method_name.into()),
                jni::objects::JValue::Object(&param_types),
            ],
        )?
        .l()?;

    env.call_method(
        &method,
        jni::jni_str!("setAccessible"),
        jni::jni_sig!((flag: boolean) -> void),
        &[jni::objects::JValue::Bool(true)],
    )?;

    let object_class = env.find_class(jni::jni_str!("java/lang/Object"))?;
    let invoke_args = env.new_object_array(1, &object_class, &url)?;
    env.call_method(
        &method,
        jni::jni_str!("invoke"),
        jni::jni_sig!((obj: java.lang.Object, args: [java.lang.Object]) -> java.lang.Object),
        &[
            jni::objects::JValue::Object(loader),
            jni::objects::JValue::Object(&invoke_args),
        ],
    )?;

    Ok(())
}
```

Note on the `Global<JObject<'static>>` / `new_local_ref` dance: the
classloader must outlive any single `attach_current_thread` callback (it's
reused across many `jvm-load-jar` calls, each a separate callback
invocation with its own local-reference scope), so it's held as a global
reference (`Global<JObject<'static>>`, obtained via `env.new_global_ref`)
in `run`'s stack frame, and converted back to a local reference
(`env.new_local_ref`) each time a callback needs to pass it to a JNI call
that expects a scoped/local object. If `Global`/`new_local_ref`/
`new_global_ref` aren't the exact names in the installed jni-rs version,
run `cargo doc --open -p jni` and search for "global ref" — the concept
(a reference that outlives one local-frame scope, convertible back to a
local one when needed) is stable across jni-rs versions even if exact type
names drift.

- [ ] **Step 2: Add the `jvm--load-jar` defun in `src/lib.rs`**

```rust
#[defun(mod_in_name = false, name = "jvm--load-jar")]
fn jvm_load_jar(env: &Env, path: String) -> Result<u64> {
    if STATUS.get() != Status::Ready {
        return env.signal(jvm_illegal_transition, (STATUS.get().as_tag(),));
    }
    let id = worker::next_request_id();
    match worker::send_command(commands::Cmd::LoadJar { id, path }) {
        Ok(()) => Ok(id),
        Err(()) => env.signal(jvm_illegal_transition, ("no running JVM worker thread",)),
    }
}
```

- [ ] **Step 3: Run `cargo build`, fixing any type-name drift**

Run: `cargo build`
Expected: succeeds. This is the highest-risk compile step in the whole
plan (most JNI type names and generic bounds per call). If specific method
names or bounds differ from what's shown, use the compiler's suggestions
and `cargo doc --open -p jni` to find the current equivalent — the overall
sequence (find the classes, build an empty `URL[]`, construct
`URLClassLoader`, build a `File`→`URI`→`URL` chain, reflectively fetch and
invoke `addURL`) is what's load-bearing, not the exact spelling of every
generic parameter.

- [ ] **Step 4: Extend `elisp/emacs-jni.el`**

Extend `jvm--dispatch-message`'s `pcase`:
```elisp
(defun jvm--dispatch-message (sexp)
  "Handle one parsed message SEXP from the JVM worker thread."
  (pcase sexp
    (`(status ,state ,_msg)
     (setq jvm--status state)
     (run-hook-with-args 'jvm-status-hook state))
    (`(load-jar-done ,id ,ok ,err)
     (when-let ((callback (alist-get id jvm--pending-load-jar)))
       (setq jvm--pending-load-jar (assq-delete-all id jvm--pending-load-jar))
       (funcall callback ok err)))
    (_ (message "emacs-jni: unrecognized channel message: %S" sexp))))
```

Add the pending-table defvar near the other defvars:
```elisp
(defvar jvm--pending-load-jar nil
  "Alist of request-id -> callback for in-flight `jvm-load-jar' calls.")
```

Insert before `(provide 'emacs-jni)`:
```elisp
(defun jvm-load-jar (path &optional callback)
  "Load the jar at PATH into the shared classloader asynchronously.
Returns a request id immediately. If CALLBACK is given, it is called as
\(CALLBACK OK ERR) once loading finishes."
  (let ((id (jvm--load-jar path)))
    (when callback
      (push (cons id callback) jvm--pending-load-jar))
    id))

(defun jvm-load-jar-sync (path &optional timeout)
  "Like `jvm-load-jar', but blocks until it finishes or TIMEOUT (default 30s).
Returns a cons (OK . ERR); signals `jvm-timeout' if TIMEOUT elapses first."
  (let* ((result :pending)
         (id (jvm-load-jar path (lambda (ok err) (setq result (cons ok err)))))
         (deadline (+ (float-time) (or timeout 30))))
    (while (eq result :pending)
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (setq jvm--pending-load-jar (assq-delete-all id jvm--pending-load-jar))
          (signal 'jvm-timeout (list 'jvm-load-jar-sync path)))))
    result))
```

- [ ] **Step 5: Create the test fixture**

`test/fixtures/Greeter.java`:
```java
public class Greeter {
    public static String greet() {
        return "hello from Greeter";
    }
}
```

- [ ] **Step 6: Update `test/run-ert-tests.sh` to build the fixture jar**

Change:
```bash
cd "$(dirname "$0")/.."

CARGO_FEATURES="${EMACS_JNI_TEST_FEATURES:-}"
```
to:
```bash
cd "$(dirname "$0")/.."

mkdir -p target/test-fixtures
javac -d target/test-fixtures test/fixtures/Greeter.java
jar cf target/test-fixtures/greeter.jar -C target/test-fixtures Greeter.class
export EMACS_JNI_GREETER_JAR="$(pwd)/target/test-fixtures/greeter.jar"

CARGO_FEATURES="${EMACS_JNI_TEST_FEATURES:-}"
```

- [ ] **Step 7: Write `test/jvm-load-jar-test.el`**

```elisp
;;; jvm-load-jar-test.el --- Tests for jvm-load-jar -*- lexical-binding: t; -*-

(require 'ert)
(require 'emacs-jni)

(ert-deftest emacs-jni-jvm-load-jar-test/loads-fixture-jar ()
  (should (eq (jvm-start-sync) 'ready))
  (let ((result (jvm-load-jar-sync (getenv "EMACS_JNI_GREETER_JAR"))))
    (should (eq (car result) t))
    (should (null (cdr result)))))

(ert-deftest emacs-jni-jvm-load-jar-test/reports-failure-for-missing-jar ()
  (should (eq (jvm-status) 'ready))
  (let ((result (jvm-load-jar-sync "/nonexistent/path/does-not-exist.jar")))
    (should (eq (car result) nil))
    (should (stringp (cdr result)))))

(provide 'jvm-load-jar-test)
;;; jvm-load-jar-test.el ends here
```

- [ ] **Step 8: Run it**

Run: `./test/run-ert-tests.sh test/jvm-load-jar-test.el`
Expected: both tests pass.

- [ ] **Step 9: Commit**

```bash
git add src/jvm_thread.rs src/lib.rs elisp/emacs-jni.el test/fixtures/Greeter.java test/run-ert-tests.sh test/jvm-load-jar-test.el
git commit -m "Add jvm-load-jar via reflective addURL on a shared classloader"
```

---

## Task 8: Crash handling

**Files:**
- Modify: `src/jvm_thread.rs` (wrap each command handler in `catch_unwind`)
- Modify: `src/commands.rs` (add a `test-support`-gated `Cmd::TestPanic`)
- Modify: `src/lib.rs` (add a `test-support`-gated `jvm--test-panic` defun)
- Modify: `test/run-ert-tests.sh` (build with `test-support` feature when
  running the crash test)
- Create: `test/jvm-crash-test.el`

**Interfaces:**
- Consumes: everything from Tasks 4-7.
- Produces: no new public API beyond the test-only `jvm--test-panic`
  defun, which must never be reachable in a normal (non-`test-support`)
  build.

- [ ] **Step 1: Add the test-only panic command, feature-gated**

In `src/commands.rs`, add:
```rust
#[derive(Debug)]
pub enum Cmd {
    Resume,
    Pause,
    Stop,
    LoadJar { id: u64, path: String },
    #[cfg(feature = "test-support")]
    TestPanic,
}
```

- [ ] **Step 2: Wrap command handling in `catch_unwind` in `src/jvm_thread.rs`**

Change the loop body in `run` from:
```rust
    for cmd in receiver {
        match cmd {
            Cmd::Resume => handle_resume(&vm),
            Cmd::Pause => handle_pause(&vm),
            Cmd::Stop => {
                handle_stop(vm);
                break;
            }
            Cmd::LoadJar { id, path } => handle_load_jar(&vm, &mut loader, id, path),
        }
    }
```
to:
```rust
    for cmd in receiver {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match cmd {
            Cmd::Resume => {
                handle_resume(&vm);
                false
            }
            Cmd::Pause => {
                handle_pause(&vm);
                false
            }
            Cmd::Stop => true, // signal "stop after this iteration" below
            Cmd::LoadJar { id, path } => {
                handle_load_jar(&vm, &mut loader, id, path);
                false
            }
            #[cfg(feature = "test-support")]
            Cmd::TestPanic => panic!("jvm--test-panic: deliberate panic for crash-handling tests"),
        }));

        match outcome {
            Ok(true) => {
                // Cmd::Stop: `vm` needs to move into handle_stop, which
                // catch_unwind's closure can't do (AssertUnwindSafe wraps
                // a Fn-like closure, not a one-shot move of `vm` itself).
                // Call it here, outside the closure, then break.
                handle_stop(vm);
                break;
            }
            Ok(false) => {}
            Err(payload) => {
                let message = panic_message(&payload);
                STATUS.finish(Status::Crashed);
                channel::send(&protocol::status_message(Status::Crashed, Some(&message)));
                worker::clear_sender();
                return;
            }
        }
    }
```

Add the helper below `create_jvm`:
```rust
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}
```

Note: moving `vm` out of the loop for `Cmd::Stop` while `vm` is borrowed
by-reference inside the `catch_unwind` closure for every other arm needs
the closure to only ever take `&vm`, never `vm` itself — the restructuring
above keeps `handle_stop(vm)` outside the closure (called after
`catch_unwind` returns, using the `Ok(true)` signal) specifically to avoid
a borrow-checker conflict between "closure borrows vm" and "match arm
consumes vm by value." If `cargo build` still rejects this arrangement,
the fallback is to make `vm` an `Option<JavaVM>` in `run`'s scope, `.take()`
it right before calling `handle_stop`, and adjust `handle_resume`/
`handle_pause`/`handle_load_jar` to accept `vm.as_ref().unwrap()`.

- [ ] **Step 3: Add the test-only defun, feature-gated**

In `src/lib.rs`, add:
```rust
#[cfg(feature = "test-support")]
#[defun(mod_in_name = false, name = "jvm--test-panic")]
fn jvm_test_panic(env: &Env) -> Result<()> {
    match worker::send_command(commands::Cmd::TestPanic) {
        Ok(()) => Ok(()),
        Err(()) => env.signal(jvm_illegal_transition, ("no running JVM worker thread",)),
    }
}
```

- [ ] **Step 4: Run `cargo build` twice (with and without the feature)**

Run: `cargo build`
Expected: succeeds, and `jvm--test-panic` / `Cmd::TestPanic` are absent
from the compiled artifact (confirm with
`nm -D target/debug/libemacs_jni.so | grep -i test_panic` — expect no
output).

Run: `cargo build --features test-support`
Expected: succeeds, and this time
`nm -D target/debug/libemacs_jni.so | grep -i test_panic` does find the
symbol.

- [ ] **Step 5: Write `test/jvm-crash-test.el`**

```elisp
;;; jvm-crash-test.el --- Tests for crash recovery -*- lexical-binding: t; -*-

(require 'ert)
(require 'emacs-jni)

(defun jvm--wait-for-status (target timeout)
  (let ((deadline (+ (float-time) timeout)))
    (while (not (eq (jvm-status) target))
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (error "Timed out waiting for status %s, got %s" target (jvm-status)))))))

(ert-deftest emacs-jni-jvm-crash-test/panic-marks-crashed-and-allows-restart ()
  (should (eq (jvm-start-sync) 'ready))
  (jvm--test-panic)
  (jvm--wait-for-status 'crashed 10)
  (should (eq (jvm-status) 'crashed))
  ;; Restarting after a crash re-creates the JVM from scratch (the old one
  ;; may be in an undefined state); this may itself fail, which is
  ;; documented as acceptable in the spec's Error Handling section, so
  ;; accept either outcome as long as it's not stuck in 'starting.
  (should (memq (jvm-start-sync) '(ready start-failed))))

(provide 'jvm-crash-test)
;;; jvm-crash-test.el ends here
```

- [ ] **Step 6: Update `test/run-ert-tests.sh` to support the feature flag for this test**

The script already reads `EMACS_JNI_TEST_FEATURES` (Task 1, Step 5) and
passes it to `cargo build`; no script changes are needed here, just invoke
it correctly in the next step.

- [ ] **Step 7: Run the crash test with the feature enabled**

Run: `EMACS_JNI_TEST_FEATURES=test-support ./test/run-ert-tests.sh test/jvm-crash-test.el`
Expected: passes.

- [ ] **Step 8: Re-run the full test suite without the feature, to confirm normal builds are unaffected**

Run: `./test/run-ert-tests.sh`
Expected: all tests except `jvm-crash-test.el`'s pass; `jvm-crash-test.el`
itself will fail to load in a non-`test-support` build since `jvm--test-panic`
won't exist — exclude it from the default run by listing the other files
explicitly:
Run: `./test/run-ert-tests.sh test/smoke-test.el test/jvm-start-test.el test/jvm-pause-test.el test/jvm-stop-test.el test/jvm-load-jar-test.el`
Expected: all pass.

- [ ] **Step 9: Commit**

```bash
git add src/jvm_thread.rs src/commands.rs src/lib.rs test/jvm-crash-test.el
git commit -m "Add catch_unwind-based crash handling and a feature-gated test hook"
```

---

## Self-Review

**Spec coverage:**
- Target platform / crate choices / single crate → Global Constraints +
  Task 1's `Cargo.toml`. ✓
- Architecture (dedicated thread, mpsc command channel, `open_channel`
  event channel) → Task 4. ✓ (with the single-persistent-thread refinement
  documented and flagged, not silently substituted)
- Pause vs. stop → Tasks 5, 6. ✓
- State machine, transient-vs-terminal status timing → Task 2
  (`StatusCell`) + Task 4/5/6's Elisp functions setting `jvm--status`
  synchronously before calling the Rust primitive. ✓
- Jar loading (shared classloader, no per-jar unload) → Task 7. ✓ (no
  `jvm-unload-jar` defun exists anywhere in this plan, matching the spec's
  explicit out-of-scope note)
- Lisp-facing API (`jvm-classpath`/`jvm-options` as `defvar`, all five
  async primitives, `jvm-status-hook`, all four `-sync` wrappers) → Tasks
  4-7. ✓ `*jvm-log*` buffer from the spec's Observability section is
  **not** implemented — see the note below; `(log ...)` messages are only
  ever sent by `handle_pause`'s error path in this plan, which
  `jvm--process-filter`/`jvm--dispatch-message` would report as an
  "unrecognized channel message" today since the `pcase` in Task 4/7 only
  handles `status` and `load-jar-done`. **Fixed inline below.**
- Wire protocol → Task 3, with the documented 3-element-status refinement.
  `log` messages are encoded (Task 3) but not consumed — see fix below.
- Error handling (gating, `crashed` recovery attempt) → Tasks 4-8. ✓
- Testing (cargo test for pure logic, ERT for integration) → every task. ✓
- Risks section (stop caveat, no per-jar unload, one-JVM-per-process) →
  reflected in code comments/docstrings throughout rather than restated
  separately, since the spec doesn't ask for a dedicated risks-tracking
  task.

**Fix applied during self-review:** the spec's Observability section
promises a `*jvm-log*` buffer fed by `(log ...)` messages, and Task 5's
`handle_pause` error path is the one place this plan actually emits a
`log` message — but no task wired the Elisp filter to handle the `log` tag
or created the buffer, so that message would have silently hit the
filter's fallback `(message "emacs-jni: unrecognized channel message...")`
branch instead. Fixing this in Task 4 (where the filter's `pcase` is first
written) rather than adding a whole new task for one buffer:

In Task 4, Step 7, the `jvm--dispatch-message` function shown should
instead be:
```elisp
(defvar jvm--log-buffer-name "*jvm-log*")

(defun jvm--dispatch-message (sexp)
  "Handle one parsed message SEXP from the JVM worker thread."
  (pcase sexp
    (`(status ,state ,_msg)
     (setq jvm--status state)
     (run-hook-with-args 'jvm-status-hook state))
    (`(log ,level ,msg)
     (with-current-buffer (get-buffer-create jvm--log-buffer-name)
       (goto-char (point-max))
       (insert (format "[%s] %s\n" level msg))))
    (_ (message "emacs-jni: unrecognized channel message: %S" sexp))))
```
(Task 7 then extends this same `pcase` with the `load-jar-done` clause, as
already written in that task's Step 4 — no change needed there.)

**Placeholder scan:** no "TBD"/"TODO"/"implement later" strings; every code
block is complete, runnable code, not a description of code. The two
places where this plan explicitly says "if X doesn't match, use the
compiler's suggestion" (Task 4 Step 6, Task 7 Step 3) are not placeholders
for missing design — they're flagging genuine external-API version risk
(jni-rs is a fast-moving crate; exact signatures were verified against
0.22.4 specifically) with a concrete, bounded fallback instruction, not an
unspecified "add error handling"-style gap.

**Type consistency check:**
- `Cmd` variants (`Resume`, `Pause`, `Stop`, `LoadJar { id: u64, path: String }`,
  test-only `TestPanic`) are introduced once in Task 4/8 and never renamed;
  every `match cmd { ... }` across Tasks 4-8 uses the same variant names.
- `Status`/`STATUS`/`as_tag()` (Task 2) are used identically in
  `protocol.rs` (Task 3), `jvm_thread.rs` (Tasks 4-8), and `lib.rs` (Tasks
  4-7) — no renamed variants.
- `worker::start_or_resume`, `worker::send_command`, `worker::clear_sender`,
  `worker::next_request_id` (Task 4) keep identical signatures everywhere
  they're called (Tasks 5-8).
- `channel::init`/`channel::send` (Task 4) are never renamed.
- Elisp: `jvm--status`, `jvm-status-hook`, `jvm--pipe-process`,
  `jvm--pending-load-jar` are defined once (Tasks 4/7) and referenced by
  the same names throughout; all four `-sync` wrappers follow the same
  `accept-process-output` + deadline + `jvm-timeout` pattern.
- Crate/lib name `emacs_jni` / package name `emacs-jni` is consistent
  across `Cargo.toml`, every `src/*.rs` file path, and
  `elisp/emacs-jni.el`'s `module-load` path (`libemacs_jni.so`).

No further issues found; no unresolved gaps remain.
