# Clojure Eval + nREPL Server (Phase 3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Evaluate Clojure code and run a real, externally-reachable nREPL
server from Emacs, built entirely on Phase 2's existing `jvm-call`/
`jvm-new` machinery.

**Architecture:** One small fix to `handle_call` (a per-`Cmd::Call`-thread's
context classloader is null by default, which breaks Clojure's own
namespace loader even though plain JNI reflection doesn't need it) plus a
thin Elisp layer that chains existing `jvm-call-sync` calls through
Clojure's own embedding facade (`clojure.java.api.Clojure`) to bootstrap
`require`/`eval`/`start-server`. No new wire-protocol messages, no new
`Cmd` variants, no new marshalling — this was proven sufficient by a real
spike (real Clojure + nREPL jars, a real embedded server, a real external
TCP client getting a correct eval result back) before this plan was
written; see the spec's "Feasibility" section for exactly what was run.

**Tech Stack:** Same as Phase 1/2 (Rust, `emacs`/`jni` crates, Emacs Lisp,
ERT), plus Clojure 1.12.0 and nREPL 1.7.0 as **user-supplied**, not
bundled, jars (see Global Constraints).

**Spec:** `docs/superpowers/specs/2026-09-02-clojure-nrepl-design.md`

## Global Constraints

- No new wire-protocol messages, no new `Cmd` variants, no new
  marshalling logic — every JNI-touching operation in this phase is an
  existing `jvm-call`/`jvm-new` call (per spec "Architecture").
- The only Rust source change in this entire plan is the context-
  classloader fix in `handle_call` (Task 1). Do not add any other new
  Rust code without a real, demonstrated need.
- Clojure/nREPL jars are user-supplied, fetched via a documented script
  into `target/clojure-jars/` (gitignored, same as `target/test-fixtures/`
  already is), never bundled or committed. Verified-working versions
  (per spec "Jar acquisition"): `org.clojure:clojure:1.12.0`,
  `org.clojure:spec.alpha:0.5.238`, `org.clojure:core.specs.alpha:0.4.74`
  (all from Maven Central, `https://repo1.maven.org/maven2`), and
  `nrepl:nrepl:1.7.0` from **Clojars** (`https://repo.clojars.org`) — not
  Maven Central; nREPL is not published there.
- Every jar-dependent test skips cleanly via `ert-skip` (not fails) when
  `EMACS_JNI_CLOJURE_JARS_DIR` is unset or doesn't exist — this repo's
  test suite must stay runnable with no network access.
- All new Lisp-facing functions return `(OK RESULT ERR)` — the exact
  3-element list shape `jvm-call-sync` already uses — never a new error
  type or a different shape. No `-sync`-vs-async split for the four new
  functions (see spec "Lisp-facing API": these are one-shot setup calls,
  not a hot-path primitive like `jvm-call` itself, so only synchronous
  forms are in scope for this plan).
- No git worktree — every task below commits directly to `master`, same
  as Phase 1/2.

---

## Task 1: Context-classloader fix + jar-fetching script

**Files:**
- Modify: `src/jvm_thread.rs` (`handle_call`)
- Create: `test/fetch-clojure-jars.sh`
- Create: `test/jvm-clojure-test.el` (shared test helpers + this task's own proof test)
- Modify: `.gitignore` — **check first**: `/target` is already a blanket rule (verify with `cat .gitignore`), so `target/clojure-jars/` needs no new entry; do not add a redundant one.

**Interfaces:**
- Produces: the fixed `handle_call` (no new public signature — same
  function, same callers, just correct behavior once a Clojure-touching
  call happens). `test/jvm-clojure-test.el`'s shared helpers
  `jvm-clojure-test--ensure-ready` (idempotent: starts the JVM and loads
  every jar in `EMACS_JNI_CLOJURE_JARS_DIR` if not already done — ERT
  does not guarantee test execution order within a file, the identical
  problem Phase 2's Task 5 already hit and fixed the same way) and
  `jvm-clojure-test--skip-unless-jars`. Tasks 2 and 3 add more
  `ert-deftest` forms to this same file, calling
  `jvm-clojure-test--ensure-ready` themselves.

- [ ] **Step 1: Write the jar-fetching script**

`test/fetch-clojure-jars.sh`:
```bash
#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-$(dirname "$0")/../target/clojure-jars}"
mkdir -p "$OUT_DIR"

MAVEN=https://repo1.maven.org/maven2
CLOJARS=https://repo.clojars.org

fetch() {
    local url="$1" out="$OUT_DIR/$2"
    if [ -f "$out" ]; then
        echo "already have $2"
        return
    fi
    echo "fetching $2"
    curl -sf -o "$out" "$url"
}

# Verified-working versions — see docs/superpowers/specs/2026-09-02-clojure-nrepl-design.md
fetch "$MAVEN/org/clojure/clojure/1.12.0/clojure-1.12.0.jar" clojure-1.12.0.jar
fetch "$MAVEN/org/clojure/spec.alpha/0.5.238/spec.alpha-0.5.238.jar" spec.alpha-0.5.238.jar
fetch "$MAVEN/org/clojure/core.specs.alpha/0.4.74/core.specs.alpha-0.4.74.jar" core.specs.alpha-0.4.74.jar
# nREPL is published to Clojars, NOT Maven Central.
fetch "$CLOJARS/nrepl/nrepl/1.7.0/nrepl-1.7.0.jar" nrepl-1.7.0.jar

echo "Jars ready in $OUT_DIR"
echo "Run Clojure/nREPL tests with:"
echo "  EMACS_JNI_CLOJURE_JARS_DIR=$OUT_DIR ./test/run-ert-tests.sh test/jvm-clojure-test.el"
```

Run: `chmod +x test/fetch-clojure-jars.sh`

- [ ] **Step 2: Run it to fetch the jars**

Run: `./test/fetch-clojure-jars.sh`
Expected: succeeds, `target/clojure-jars/` contains 4 jars. If a
previous session already downloaded these into `.probe/clojure-jars/`
(a throwaway spike directory, not part of this plan), you can instead
copy them: `mkdir -p target/clojure-jars && cp .probe/clojure-jars/*.jar target/clojure-jars/`
— either source is fine, the script exists so this is reproducible for
anyone else too.

- [ ] **Step 3: Apply the context-classloader fix**

In `src/jvm_thread.rs`, find `handle_call`'s body — specifically the
start of the `vm.attach_current_thread(|env| { ... })` closure, right
before the `let (class, instance) = match &target { ... }` line. Insert
this immediately after the closure opens (before that `match`):

```rust
        // A freshly attached thread's context classloader is null. Plain
        // JNI class resolution (find_class/loadClass, used just below)
        // doesn't need it, but Clojure's own namespace loader
        // (clojure.lang.RT, used internally by `require`) does: it loads
        // .clj source files as classpath *resources*, not classes, and
        // that resolution path falls back through
        // Thread.currentThread().getContextClassLoader(). Left null, the
        // first touch of Clojure's runtime throws
        // ExceptionInInitializerError before clojure.core even finishes
        // loading — verified empirically during Phase 3's design spike
        // (see docs/superpowers/specs/2026-09-02-clojure-nrepl-design.md,
        // "Feasibility"). Harmless for calls that never touch Clojure.
        if let Some(global) = &loader {
            let loader_local = env.new_local_ref(global.as_obj())?;
            let thread_class = env.find_class(jni::jni_str!("java/lang/Thread"))?;
            let current_thread = env
                .call_static_method(&thread_class, jni::jni_str!("currentThread"), jni::jni_sig!(() -> java.lang.Thread), &[])?
                .l()?;
            env.call_method(
                &current_thread,
                jni::jni_str!("setContextClassLoader"),
                jni::jni_sig!((cl: java.lang.ClassLoader) -> void),
                &[jni::objects::JValue::Object(&loader_local)],
            )?;
        }
```

`handle_call`'s existing `loader: Option<Arc<jni::refs::Global<jni::objects::JObject<'static>>>>`
parameter (already threaded in by Phase 2's Task 5) is exactly what this
needs — no signature change.

- [ ] **Step 4: Run `cargo build`**

Run: `cargo build`
Expected: succeeds — this is the same JNI call shape (`find_class` +
`call_static_method` + `call_method`) already used throughout
`jvm_thread.rs`, so it should compile as written. If `jni::objects::JValue`
(as opposed to `JValueOwned`, used elsewhere in this file) doesn't have
the exact constructor shown, check the compiler's error and the vendored
`jni-0.22.4` source (`~/.cargo/registry/src/*/jni-0.22.4/`) — this exact
sequence was verified to compile during the design spike, but double-check
against whatever's actually in `Cargo.lock` now in case anything shifted.

- [ ] **Step 5: Write `test/jvm-clojure-test.el` with the shared helpers and this task's proof test**

```elisp
;;; jvm-clojure-test.el --- Tests for Clojure eval + nREPL -*- lexical-binding: t; -*-

(require 'ert)
(require 'emacs-jni)

(defvar jvm-clojure-test--jars-dir (getenv "EMACS_JNI_CLOJURE_JARS_DIR")
  "Directory containing Clojure/nREPL jars, or nil if unset.
Set via `test/fetch-clojure-jars.sh' and the EMACS_JNI_CLOJURE_JARS_DIR
environment variable — tests in this file skip cleanly, not fail, when
this isn't configured, so the suite stays runnable with no network
access.")

(defvar jvm-clojure-test--ready nil
  "Set to t once the JVM is started and jars are loaded, so every test
in this file can call `jvm-clojure-test--ensure-ready' idempotently
regardless of ERT's (non-definition-order) execution order — the exact
problem Phase 2's Task 5 already hit for `jvm-call-test.el'.")

(defun jvm-clojure-test--skip-unless-jars ()
  (unless (and jvm-clojure-test--jars-dir (file-directory-p jvm-clojure-test--jars-dir))
    (ert-skip "Set EMACS_JNI_CLOJURE_JARS_DIR (see test/fetch-clojure-jars.sh) to run this test")))

(defun jvm-clojure-test--ensure-ready ()
  (jvm-clojure-test--skip-unless-jars)
  (unless jvm-clojure-test--ready
    (should (eq (jvm-start-sync) 'ready))
    (dolist (jar (directory-files jvm-clojure-test--jars-dir t "\\.jar\\'"))
      (should (car (jvm-load-jar-sync jar))))
    (setq jvm-clojure-test--ready t)))

(ert-deftest emacs-jni-jvm-clojure-test/context-classloader-fix-lets-clojure-bootstrap ()
  "Proves Task 1's fix: without it, this fails with
ExceptionInInitializerError the first time Clojure's runtime is touched
\(see the spec's Feasibility section for the exact failure)."
  (jvm-clojure-test--ensure-ready)
  (let ((require-result (jvm-call-sync "clojure.java.api.Clojure" "var" (list "clojure.core" "require"))))
    (should (eq (nth 0 require-result) t))
    (let* ((require-fn (nth 1 require-result))
           (sym-result (jvm-call-sync "clojure.java.api.Clojure" "read" (list "clojure.set"))))
      (should (eq (nth 0 sym-result) t))
      (should (equal (jvm-call-sync require-fn "invoke" (list (nth 1 sym-result))) '(t nil nil))))))

(provide 'jvm-clojure-test)
;;; jvm-clojure-test.el ends here
```

- [ ] **Step 6: Run it**

Run: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-clojure-test.el`
Expected: 1 test passes. If it fails with `ExceptionInInitializerError`
again, the fix from Step 3 isn't taking effect — check that `handle_call`
actually has a `Some(global)` `loader` (i.e. a jar was really loaded
before this call) and that the edit landed in the right closure.

- [ ] **Step 7: Confirm the full existing suite still passes**

Run: `cargo test --lib -- --test-threads=1 && EMACS_JNI_TEST_FEATURES=test-support ./test/run-ert-tests.sh`
Expected: everything from Phase 1/2 still passes (this task's only
production-code change is additive — a new `if let Some(...)` block that
does nothing when `loader` is `None`, which is every existing test's
case).

- [ ] **Step 8: Commit**

```bash
git add src/jvm_thread.rs test/fetch-clojure-jars.sh test/jvm-clojure-test.el
git commit -m "Fix context classloader on Cmd::Call threads, letting Clojure's require work"
```

---

## Task 2: `jvm-clojure-require` and `jvm-clojure-eval-string`

**Files:**
- Modify: `elisp/emacs-jni.el`
- Modify: `test/jvm-clojure-test.el`

**Interfaces:**
- Consumes: `jvm-call-sync` (Phase 2, existing), `jvm-clojure-test--ensure-ready` (Task 1).
- Produces: `jvm-clojure-require (namespace &optional timeout)`,
  `jvm-clojure-eval-string (code &optional timeout)` — both return
  `(OK RESULT ERR)`. Internal helpers `jvm-clojure--var`,
  `jvm-clojure--read`, `jvm-clojure--invoke-with-form` (Task 3 reuses
  `jvm-clojure--var` and `jvm-clojure--read` directly — do not rename
  them without updating Task 3).

- [ ] **Step 1: Add the Clojure bootstrap helpers to `elisp/emacs-jni.el`**

Insert before `(provide 'emacs-jni)`:

```elisp
(defun jvm-clojure--var (ns name)
  "Resolve the wrapped IFn for NS/NAME via `clojure.java.api.Clojure/var'.
Returns (OK IFN ERR), same shape as `jvm-call-sync'."
  (jvm-call-sync "clojure.java.api.Clojure" "var" (list ns name)))

(defun jvm-clojure--read (form-string)
  "Parse FORM-STRING as Clojure data via `clojure.java.api.Clojure/read'.
Returns (OK DATA ERR), same shape as `jvm-call-sync'."
  (jvm-call-sync "clojure.java.api.Clojure" "read" (list form-string)))

(defun jvm-clojure--invoke-with-form (var-ns var-name form-string &optional timeout)
  "Resolve VAR-NS/VAR-NAME, read FORM-STRING as Clojure data, and invoke
the resolved IFn with it as its sole argument. Returns (OK RESULT ERR) --
whichever step fails first short-circuits the rest. Shared plumbing for
`jvm-clojure-require' and `jvm-clojure-eval-string', both of which are
\"resolve a var, read one form, invoke\"."
  (let ((fn (jvm-clojure--var var-ns var-name)))
    (if (not (car fn))
        fn
      (let ((form (jvm-clojure--read form-string)))
        (if (not (car form))
            form
          (jvm-call-sync (nth 1 fn) "invoke" (list (nth 1 form)) timeout))))))

(defun jvm-clojure-require (namespace &optional timeout)
  "Require the Clojure NAMESPACE (a string, e.g. \"clojure.set\").
Returns (OK RESULT ERR), same shape as `jvm-call-sync'."
  (jvm-clojure--invoke-with-form "clojure.core" "require" namespace timeout))

(defun jvm-clojure-eval-string (code &optional timeout)
  "Evaluate CODE (a string of Clojure source) and return its result.
Returns (OK RESULT ERR), same shape as `jvm-call-sync'.

Each call may run on a different underlying JVM thread (Phase 2's
per-call dispatch model), so `*ns*' set by a previous call (e.g. via
`in-ns') does not persist to the next one -- fold namespace switches into
CODE itself when it matters. See
docs/superpowers/specs/2026-09-02-clojure-nrepl-design.md, \"Known
limitation\". Non-scalar Clojure return values (keywords, vectors, maps)
come back as an opaque wrapped JVM object, same limitation as any other
non-scalar `jvm-call' result."
  (jvm-clojure--invoke-with-form "clojure.core" "eval" code timeout))
```

- [ ] **Step 2: Run `cargo build` (sanity — no Rust changed, just confirming nothing else broke)**

Run: `cargo build`
Expected: succeeds (this step touches no Rust code; this is a checkpoint,
not expected to catch anything).

- [ ] **Step 3: Add tests to `test/jvm-clojure-test.el`**

Insert before `(provide 'jvm-clojure-test)`:

```elisp
(ert-deftest emacs-jni-jvm-clojure-test/require-a-namespace ()
  (jvm-clojure-test--ensure-ready)
  (should (equal (jvm-clojure-require "clojure.set") '(t nil nil))))

(ert-deftest emacs-jni-jvm-clojure-test/eval-string-returns-marshalled-scalar ()
  (jvm-clojure-test--ensure-ready)
  (should (equal (jvm-clojure-eval-string "(+ 40 (* 2 1))") '(t 42 nil))))

(ert-deftest emacs-jni-jvm-clojure-test/eval-string-propagates-clojure-error ()
  (jvm-clojure-test--ensure-ready)
  (let ((result (jvm-clojure-eval-string "(/ 1 0)")))
    (should (eq (nth 0 result) nil))
    (should (string-match-p "Divide by zero" (nth 2 result)))))
```

- [ ] **Step 4: Run it**

Run: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-clojure-test.el`
Expected: 4 tests pass (this task's 3 plus Task 1's).

- [ ] **Step 5: Commit**

```bash
git add elisp/emacs-jni.el test/jvm-clojure-test.el
git commit -m "Add jvm-clojure-require and jvm-clojure-eval-string"
```

---

## Task 3: `jvm-nrepl-start`/`jvm-nrepl-stop` and a real external protocol test

**Files:**
- Modify: `elisp/emacs-jni.el`
- Modify: `test/jvm-clojure-test.el`

**Interfaces:**
- Consumes: `jvm-clojure-require`, `jvm-clojure--var`, `jvm-clojure--read` (Task 2).
- Produces: `jvm-nrepl-start (&optional port timeout)` -> `(OK RESULT ERR)`
  where `RESULT`, if `OK`, is `(cons ACTUAL-PORT SERVER)` (`ACTUAL-PORT`
  an integer, `SERVER` a wrapped JVM object); `jvm-nrepl-stop (server
  &optional timeout)` -> `(OK RESULT ERR)`.

- [ ] **Step 1: Add `jvm-nrepl-start`/`jvm-nrepl-stop` to `elisp/emacs-jni.el`**

Insert before `(provide 'emacs-jni)` (after the Task 2 additions):

```elisp
(defun jvm-nrepl-start (&optional port timeout)
  "Start an nREPL server (requiring `nrepl.server' first if needed).
PORT defaults to 0, letting the OS pick a free port. Returns
\(OK RESULT ERR); if OK, RESULT is (cons ACTUAL-PORT SERVER) -- the real
bound port and a wrapped JVM object `jvm-nrepl-stop' needs later."
  (catch 'jvm-nrepl-start
    (let ((required (jvm-clojure-require "nrepl.server" timeout)))
      (unless (car required) (throw 'jvm-nrepl-start required))
      (let ((start-fn (jvm-clojure--var "nrepl.server" "start-server")))
        (unless (car start-fn) (throw 'jvm-nrepl-start start-fn))
        (let ((port-kw (jvm-clojure--read ":port")))
          (unless (car port-kw) (throw 'jvm-nrepl-start port-kw))
          (let ((server (jvm-call-sync (nth 1 start-fn) "invoke"
                                        (list (nth 1 port-kw) (or port 0))
                                        timeout)))
            (unless (car server) (throw 'jvm-nrepl-start server))
            (let ((bound-port (jvm-call-sync (nth 1 port-kw) "invoke"
                                              (list (nth 1 server)) timeout)))
              (unless (car bound-port) (throw 'jvm-nrepl-start bound-port))
              (list t (cons (nth 1 bound-port) (nth 1 server)) nil))))))))

(defun jvm-nrepl-stop (server &optional timeout)
  "Stop SERVER (the cdr of `jvm-nrepl-start''s RESULT). Returns
\(OK RESULT ERR), same shape as `jvm-call-sync'."
  (let ((stop-fn (jvm-clojure--var "nrepl.server" "stop-server")))
    (if (not (car stop-fn))
        stop-fn
      (jvm-call-sync (nth 1 stop-fn) "invoke" (list server) timeout))))
```

- [ ] **Step 2: Run `cargo build` (sanity checkpoint, no Rust touched)**

Run: `cargo build`
Expected: succeeds.

- [ ] **Step 3: Add the real external-connection test**

This is the test that actually proves the deliverable — a real external
process (not Emacs's own network stack) speaking real bencode to a real
running server, exactly as verified during the design spike. Insert
before `(provide 'jvm-clojure-test)` in `test/jvm-clojure-test.el`:

```elisp
(ert-deftest emacs-jni-jvm-nrepl-test/start-serves-real-bencode-protocol ()
  "Starts a real nREPL server and connects to it from a SEPARATE OS
process (bash + /dev/tcp) speaking the real bencode wire protocol -- this
is deliberately not Emacs's own network stack, since that's what an
actual external nREPL client does, and it's the exact mechanism verified
working during the Phase 3 design spike (see the spec's Feasibility
section)."
  (jvm-clojure-test--ensure-ready)
  (let ((start-result (jvm-nrepl-start)))
    (should (car start-result))
    (let* ((port (car (nth 1 start-result)))
           (server (cdr (nth 1 start-result)))
           (code "(+ 40 (* 2 1))")
           (script (format
                    "exec 3<>/dev/tcp/127.0.0.1/%d; printf '%%s' 'd2:op4:eval4:code%d:%s2:id1:1e' >&3; timeout 5 cat <&3; exec 3<&-; exec 3>&-"
                    port (length code) code))
           (response (with-temp-buffer
                       (call-process "bash" nil t nil "-c" script)
                       (buffer-string))))
      ;; Bencode has no quoting: a correct eval response contains the
      ;; literal substring "5:value2:42" (the "value" key's 5-char length
      ;; prefix, then the "42" result's 2-char length prefix) -- this is
      ;; the exact response shape observed during the design spike, not a
      ;; guess.
      (should (string-match-p "5:value2:42" response))
      (jvm-nrepl-stop server))))
```

- [ ] **Step 4: Run it**

Run: `EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-clojure-test.el`
Expected: 5 tests pass (this task's plus Tasks 1-2's). If `bash`'s
`/dev/tcp` isn't available in this environment, this test will fail with
a clear bash error (e.g. "No such file or directory" on the `exec 3<>`
line) rather than silently passing — if that happens, this is a real
environment limitation to report, not something to route around with a
different silent-failure mode.

- [ ] **Step 5: Run the full suite to confirm nothing regressed**

Run: `cargo test --lib -- --test-threads=1 && EMACS_JNI_TEST_FEATURES=test-support ./test/run-ert-tests.sh`
Expected: everything passes (Clojure-dependent tests only run when
`EMACS_JNI_CLOJURE_JARS_DIR` is set; without it they skip cleanly — also
worth confirming once: run `./test/run-ert-tests.sh test/jvm-clojure-test.el`
with `EMACS_JNI_CLOJURE_JARS_DIR` *unset*, and confirm all 5 tests report
as skipped, not failed, not silently passing).

- [ ] **Step 6: Commit**

```bash
git add elisp/emacs-jni.el test/jvm-clojure-test.el
git commit -m "Add jvm-nrepl-start/jvm-nrepl-stop, verified against a real external bencode client"
```

---

## Task 4: Documentation and final verification

**Files:**
- Modify: `README.md`
- Modify: `HANDOFF.md`

**Interfaces:** none (docs only).

- [ ] **Step 1: Run the complete test suite from a clean build**

Run: `cargo build && cargo test --lib -- --test-threads=1 && EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" EMACS_JNI_TEST_FEATURES=test-support ./test/run-ert-tests.sh`
Expected: everything passes, including all 5 Clojure/nREPL tests.

- [ ] **Step 2: Manual smoke test — start a real nREPL server and connect with a real client if one is available**

From the repo root:
```
cargo build
EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" emacs -Q -L elisp
```
In `*scratch*`:
```elisp
(require 'emacs-jni)
(jvm-start-sync)
(dolist (jar (directory-files (getenv "EMACS_JNI_CLOJURE_JARS_DIR") t "\\.jar\\'"))
  (jvm-load-jar-sync jar))
(jvm-nrepl-start 7888)
;; => (t (7888 . #<user-ptr ...>) nil)
```
Then, from a separate terminal, connect with whatever real nREPL client
is available in this environment (`lein repl :connect localhost:7888`,
`clj -Sdeps '{}' -e '...'`-based client, a raw `bash`/`/dev/tcp` bencode
exchange like the automated test uses, or an editor's nREPL client if
one is installed) and evaluate something. Confirm it works. If no real
nREPL client tool is available in this environment, the automated test
from Task 3 (real external process, real bencode) is the closest
equivalent and already covers this — note in your report which you did.

- [ ] **Step 3: Update `README.md`**

Move Phase 3 from "not started" to "done" in the Roadmap section (mirror
the exact style Phase 2's own entry already uses there). Add a short
"Clojure & nREPL" section (mirroring the existing Quickstart/Interop
sections' style) covering: how to fetch the jars
(`./test/fetch-clojure-jars.sh`), and a minimal end-to-end example using
`jvm-clojure-eval-string` and `jvm-nrepl-start`, using the exact commands
verified in Step 2.

- [ ] **Step 4: Write a fresh `HANDOFF.md`**

Following the existing structure (which this replaces): "Where things
stand" (Phase 3 done — summarize what shipped, and explicitly call out
the context-classloader finding as the one real Rust-level surprise this
phase needed, the way prior HANDOFFs documented their own findings),
"Known rough edges" (at minimum: `*ns*` doesn't persist across separate
`jvm-clojure-eval-string` calls — see spec; non-scalar Clojure return
values come back as opaque handles — see spec; nREPL server defaults are
unhardened — loopback-only usage assumed, no auth/TLS, per spec "Out of
scope"; anything else you found during implementation), "What's next"
(the original three-phase scope is now complete — note this plainly;
if there's an obvious follow-on, like packaging/distribution or richer
Clojure value marshalling, mention it as a possibility, not a plan).

- [ ] **Step 5: Commit**

```bash
git add README.md HANDOFF.md
git commit -m "Phase 3 complete: update README and HANDOFF"
```
