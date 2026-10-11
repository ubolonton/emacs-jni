# Packaging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A package.el archive (`jvm-<version>.tar`), mise tasks for build/fixtures/test/package,
Clojure jars downloaded on demand by `jvm-clj.el`, and shorter tutorials.

**Architecture:** `jvm.el` and `jvm-clj.el` default to the installed-package layout (module and
`clojure/` next to the `.el` files). A checkout overrides two variables via
`scripts/dev-paths.el`. `jvm-clj.el` fetches pinned jars into the Maven local repository and
checks sha256. Shell logic lives in `scripts/`; mise tasks chain them.

**Tech Stack:** Emacs Lisp (package.el, url-handlers), bash, mise tasks, cargo.

**Spec:** `docs/superpowers/specs/2026-10-11-packaging-design.md`

## Global Constraints

- Package name `jvm`. Archive `target/package/jvm-<version>.tar`, version from `Cargo.toml`.
- The module keeps cargo's file name: `lib` prefix except on Windows, `emacs_jni`,
  `module-file-suffix`.
- `jvm-module-directory` is public. `jvm-clj--source-dir` stays private (user decision).
- `jvm-clj-dependencies` is public; entries are plists `:group :artifact :version :repository
  :sha256`.
- `jvm-clj-maven-local-repo` default `~/.m2/repository` (layout of `clj`/tools.deps and Maven).
- Removed: `EMACS_JNI_MODULE_PATH`, `jvm--module-path`, `EMACS_JNI_CLOJURE_JARS_DIR`,
  `test/fetch-clojure-jars.sh`.
- Debug profile everywhere except `mise run package` (release).
- Docs: ASD-STE100, concise, senior-engineer audience. Comments wrap at 100 columns in Rust; Lisp
  follows existing file style (fill to ~70-80).
- Commits: conventional commits, capitalized subject text.

## File map

| File | Change |
|---|---|
| `elisp/jvm.el` | `jvm-module-directory`, `jvm--module-file`; drop `jvm--module-path`. |
| `elisp/jvm-clj.el` | `jvm-clj-dependencies`, `jvm-clj-maven-local-repo`, `jvm-clj-ensure-jars`, `jvm-clj--classpath`; `jvm-clj--source-dir` default next to file. |
| `scripts/dev-paths.el` | New. Sets `load-path`, `jvm-module-directory`, `jvm-clj--source-dir` for a checkout. |
| `scripts/build-fixtures.sh` | New. `greeter.jar`, `calc.jar` (moved from `run-ert-tests.sh`). |
| `scripts/test.sh` | New. `cargo test --lib` when no args, then `test/run-ert-tests.sh "$@"`. |
| `scripts/package.sh` | New. Release build + tar. |
| `scripts/package-test.el` | New. Installs the tar in a scratch `package-user-dir` and smoke-tests it. |
| `.config/mise/config.toml` | Tasks `build`, `fixtures`, `test`, `package`, `package-test`. |
| `test/run-ert-tests.sh` | Use `scripts/build-fixtures.sh` and `-l scripts/dev-paths.el`. |
| `test/jvm-clj-jars-test.el` | New. Tests for `jvm-clj-ensure-jars` with `file://` repos. |
| `test/jvm-clj*-test.el`, `test/jvm-symbol-marshal-test.el` | Use `jvm-clj-ensure-jars`; no skip. |
| `test/fetch-clojure-jars.sh` | Delete. |
| `README.md`, `docs/tutorials/0{1,2}-*.md`, `docs/reference/{configuration,lisp-api}.md`, `docs/explanation/implementation.md`, `docs/how-to-guides/access-lisp-from-clojure.md` | Update. |

---

### Task 1: Module directory and dev paths

**Files:**
- Modify: `elisp/jvm.el:17-25`
- Create: `scripts/dev-paths.el`
- Modify: `test/run-ert-tests.sh` (drop `EMACS_JNI_MODULE_PATH`, load dev paths)
- Modify: `docs/reference/lisp-api.md` (Variables table), `docs/reference/configuration.md`
  (env var table, configuration variables)

**Interfaces:**
- Produces: `jvm-module-directory` (string, directory). `scripts/dev-paths.el` (load with `-l`),
  which also sets `jvm-clj--source-dir` to `<root>/clojure/` (consumed in Task 3).

- [ ] **Step 1: Replace the module path logic in `elisp/jvm.el`**

Replace the `jvm--module-path` defvar and the `module-load` form with:

```elisp
(defvar jvm-module-directory
  (file-name-directory (or load-file-name buffer-file-name))
  "Directory that holds the emacs-jni dynamic module.
The default, the directory of this file, is the installed-package
layout.  Set it before loading `jvm' to use another build, e.g. a
checkout's \"target/debug/\" (see scripts/dev-paths.el).")

(defun jvm--module-file ()
  "Return the path of the dynamic module in `jvm-module-directory'.
The file name is the one that cargo gives a cdylib on this platform."
  (expand-file-name (concat (if (eq system-type 'windows-nt) "" "lib")
                            "emacs_jni" module-file-suffix)
                    jvm-module-directory))

(unless (featurep 'emacs-jni)
  (module-load (jvm--module-file)))
```

- [ ] **Step 2: Create `scripts/dev-paths.el`**

```elisp
;;; dev-paths.el --- Use jvm and jvm-clj from a source checkout -*- lexical-binding: t; -*-

;;; Commentary:

;; Load this file before `jvm' to use the checkout's Lisp files, debug
;; module, and Clojure sources instead of an installed package:
;;
;;   emacs -Q -l scripts/dev-paths.el

;;; Code:

(let ((root (expand-file-name
             ".." (file-name-directory (or load-file-name buffer-file-name)))))
  (add-to-list 'load-path (expand-file-name "elisp" root))
  (setq jvm-module-directory (expand-file-name "target/debug/" root)
        jvm-clj--source-dir (expand-file-name "clojure/" root)))

;;; dev-paths.el ends here
```

(`file-name-parent-directory` needs Emacs 29; the floor is 28.)

- [ ] **Step 3: Update `test/run-ert-tests.sh`**

Delete the `export EMACS_JNI_MODULE_PATH=...` line. Change the emacs invocation to:

```bash
    if ! emacs -Q --batch -l scripts/dev-paths.el -l "$f" -f ert-run-tests-batch-and-exit; then
```

- [ ] **Step 4: Run the non-Clojure ERT suite**

Run: `./test/run-ert-tests.sh test/jvm-start-test.el test/jvm-call-test.el test/jvm-load-jar-test.el`
Expected: all pass. Also confirm a wrong directory fails clearly:
`emacs -Q --batch -L elisp --eval '(setq jvm-module-directory "/nonexistent/")' -l jvm`
Expected: error naming `/nonexistent/libemacs_jni.so`.

- [ ] **Step 5: Update docs**

`lisp-api.md` Variables table: replace the `jvm--module-path` row with
`| jvm-module-directory | jvm | string | directory of jvm.el | Directory of the dynamic module. Read once, at load time. Set it before (require 'jvm). |`.
`configuration.md`: delete the `EMACS_JNI_MODULE_PATH` row; in "Configuration variables" replace
`jvm--module-path` with `jvm-module-directory`.

- [ ] **Step 6: Commit**

```bash
git add elisp/jvm.el scripts/dev-paths.el test/run-ert-tests.sh docs/reference
git commit -m "feat(elisp): Load the module from jvm-module-directory"
```

### Task 2: mise tasks for build, fixtures, test

**Files:**
- Create: `scripts/build-fixtures.sh`, `scripts/test.sh`
- Modify: `test/run-ert-tests.sh`, `.config/mise/config.toml`, `README.md` (Testing section)

**Interfaces:**
- Consumes: `scripts/dev-paths.el` (Task 1).
- Produces: `mise run build`, `mise run fixtures`, `mise run test [ERT files...]`. Extra task
  args go to the last `run` command (verified with mise 2026.8.10).

- [ ] **Step 1: Create `scripts/build-fixtures.sh`** (executable)

```bash
#!/usr/bin/env bash
# Build the test fixture jars into target/test-fixtures/. Needs the shim classes that `cargo
# build` puts in target/emacs-jni-shim-classes/ (Calc.java uses them).
set -euo pipefail

cd "$(dirname "$0")/.."

OUT_DIR=target/test-fixtures
mkdir -p "$OUT_DIR"
javac -d "$OUT_DIR" test/fixtures/Greeter.java
jar cf "$OUT_DIR/greeter.jar" -C "$OUT_DIR" Greeter.class
javac -cp target/emacs-jni-shim-classes -d "$OUT_DIR" test/fixtures/Calc.java
jar cf "$OUT_DIR/calc.jar" -C "$OUT_DIR" Calc.class
```

- [ ] **Step 2: Use it in `test/run-ert-tests.sh`**

Remove the four greeter/calc `javac`/`jar` lines and the `mkdir`. After the `cargo build` block
add `scripts/build-fixtures.sh`. Keep `export EMACS_JNI_GREETER_JAR="$(pwd)/target/test-fixtures/greeter.jar"`.

- [ ] **Step 3: Create `scripts/test.sh`** (executable)

```bash
#!/usr/bin/env bash
# With no arguments, run the Rust unit tests and every ERT file. With arguments, run only those
# ERT files.
set -euo pipefail

cd "$(dirname "$0")/.."

if [ $# -eq 0 ]; then
    cargo test --lib
fi
exec test/run-ert-tests.sh "$@"
```

- [ ] **Step 4: Add tasks to `.config/mise/config.toml`**

```toml
[tasks.build]
description = "Build the debug module"
run = "cargo build"

[tasks.fixtures]
description = "Build the test fixture jars"
depends = ["build"]
run = "scripts/build-fixtures.sh"

[tasks.test]
description = "Run all tests, or only the given ERT files"
run = "scripts/test.sh"
```

- [ ] **Step 5: Verify**

Run: `mise run fixtures && ls target/test-fixtures/*.jar` → `calc.jar greeter.jar`.
Run: `mise run test test/jvm-call-test.el` → only that file runs, passes.

- [ ] **Step 6: README Testing section**

Replace the command block with `mise run test`, `mise run test test/jvm-call-test.el`, and
`EMACS_JNI_TEST_FEATURES=test-support mise run test test/jvm-crash-test.el`. Keep the
Clojure-jar paragraph for Task 3 to rewrite.

- [ ] **Step 7: Commit**

```bash
git add scripts .config/mise/config.toml test/run-ert-tests.sh README.md
git commit -m "dev(mise): Add build, fixtures, and test tasks"
```

### Task 3: Download and cache Clojure jars

**Files:**
- Modify: `elisp/jvm-clj.el` (defvars at top, start functions)
- Create: `test/jvm-clj-jars-test.el`
- Modify: `test/jvm-clj-test.el`, `test/jvm-clj-direct-test.el`, `test/jvm-clj-eval-test.el`,
  `test/jvm-clj-eval-direct-test.el`, `test/jvm-symbol-marshal-test.el`
- Delete: `test/fetch-clojure-jars.sh`
- Modify: `README.md`, `docs/reference/{lisp-api,configuration}.md`,
  `docs/explanation/implementation.md:215`, `docs/how-to-guides/access-lisp-from-clojure.md:15`

**Interfaces:**
- Consumes: `jvm-clj--source-dir` set by `scripts/dev-paths.el` (Task 1).
- Produces: `jvm-clj-dependencies`, `jvm-clj-maven-local-repo`, `(jvm-clj-ensure-jars)` → list
  of absolute jar paths, `(jvm-clj--classpath)` → classpath list for start.

- [ ] **Step 1: Check `file://` URLs with `url-copy-file`**

Run: `emacs -Q --batch --eval '(progn (require (quote url-handlers)) (url-copy-file "file:///etc/hostname" "/home/ubolonton/Programming/projects/emacs-jni/target/u.txt" t) (princ (with-temp-buffer (insert-file-contents "/home/ubolonton/Programming/projects/emacs-jni/target/u.txt") (buffer-string))))'`
Expected: the host name. If the content differs (MIME headers), the tests in Step 2 must use a
local HTTP server instead; stop and report.

- [ ] **Step 2: Write the failing tests `test/jvm-clj-jars-test.el`**

```elisp
;;; jvm-clj-jars-test.el --- Tests for jvm-clj-ensure-jars -*- lexical-binding: t; -*-

;; Each test serves a fake artifact from a `file://' repository and uses a
;; temporary local repository, so no network access occurs.

(require 'ert)
(require 'jvm-clj)

(defconst jvm-clj-jars-test--content "fake jar content"
  "Content of the fake artifact.")

(defmacro jvm-clj-jars-test--with-repos (sha256 &rest body)
  "Run BODY with a fake remote repository and an empty local repository.
The remote holds one artifact, example:demo:1.0.  `jvm-clj-dependencies'
lists it with SHA256.  BODY can use `remote' and `local'."
  (declare (indent 1))
  `(let* ((remote (make-temp-file "jvm-clj-remote-" t))
          (local (make-temp-file "jvm-clj-local-" t))
          (jvm-clj-maven-local-repo local)
          (jvm-clj-dependencies
           (list (list :group "example" :artifact "demo" :version "1.0"
                       :repository (concat "file://" remote)
                       :sha256 ,sha256))))
     (unwind-protect
         (progn
           (make-directory (expand-file-name "example/demo/1.0" remote) t)
           (with-temp-file (expand-file-name "example/demo/1.0/demo-1.0.jar" remote)
             (insert jvm-clj-jars-test--content))
           ,@body)
       (delete-directory remote t)
       (delete-directory local t))))

(ert-deftest emacs-jni-jvm-clj-jars-test/downloads-into-maven-layout ()
  (jvm-clj-jars-test--with-repos (secure-hash 'sha256 jvm-clj-jars-test--content)
    (let ((expected (expand-file-name "example/demo/1.0/demo-1.0.jar" local)))
      (should (equal (jvm-clj-ensure-jars) (list expected)))
      (should (file-exists-p expected)))))

(ert-deftest emacs-jni-jvm-clj-jars-test/reuses-cached-jar ()
  (jvm-clj-jars-test--with-repos (secure-hash 'sha256 jvm-clj-jars-test--content)
    (jvm-clj-ensure-jars)
    (delete-directory (expand-file-name "example" remote) t)
    (should (= (length (jvm-clj-ensure-jars)) 1))))

(ert-deftest emacs-jni-jvm-clj-jars-test/rejects-checksum-mismatch ()
  (jvm-clj-jars-test--with-repos (make-string 64 ?0)
    (should-error (jvm-clj-ensure-jars))
    (should-not (directory-files-recursively local "\\.jar\\'"))))

(ert-deftest emacs-jni-jvm-clj-jars-test/rejects-corrupt-cached-jar ()
  (jvm-clj-jars-test--with-repos (secure-hash 'sha256 jvm-clj-jars-test--content)
    (let ((cached (expand-file-name "example/demo/1.0/demo-1.0.jar" local)))
      (make-directory (file-name-directory cached) t)
      (with-temp-file cached (insert "corrupt"))
      (should-error (jvm-clj-ensure-jars)))))

(provide 'jvm-clj-jars-test)
;;; jvm-clj-jars-test.el ends here
```

- [ ] **Step 3: Run, expect failure**

Run: `mise run test test/jvm-clj-jars-test.el`
Expected: FAIL, `jvm-clj-ensure-jars` void.

- [ ] **Step 4: Implement in `elisp/jvm-clj.el`**

After `(require 'jvm)` add `(require 'url-handlers)`. Replace the `jvm-clj-classpath` and
`jvm-clj--source-dir` defvars with:

```elisp
(defvar jvm-clj-classpath nil
  "Jar files or directories added to the JVM classpath by `jvm-clj-start'.
If nil, `jvm-clj-start' uses `jvm-clj-ensure-jars', which downloads
`jvm-clj-dependencies'.  `jvm-clj-start' also adds this package's
`clojure'.")

(defvar jvm-clj-dependencies
  '((:group "org.clojure" :artifact "clojure" :version "1.12.0"
     :repository "https://repo1.maven.org/maven2"
     :sha256 "c45333006441a059ea9fdb1341fc6c1f40b921a10dccd82665311e48a0384763")
    (:group "org.clojure" :artifact "spec.alpha" :version "0.5.238"
     :repository "https://repo1.maven.org/maven2"
     :sha256 "94cd99b6ea639641f37af4860a643b6ed399ee5a8be5d717cff0b663c8d75077")
    (:group "org.clojure" :artifact "core.specs.alpha" :version "0.4.74"
     :repository "https://repo1.maven.org/maven2"
     :sha256 "eb73ac08cf49ba840c88ba67beef11336ca554333d9408808d78946e0feb9ddb")
    ;; nREPL is published to Clojars, not Maven Central.
    (:group "nrepl" :artifact "nrepl" :version "1.7.0"
     :repository "https://repo.clojars.org"
     :sha256 "d07390277190e0a974e1497fd4485005a4964f9f556a50bd530509f48ba65c1b"))
  "Jars that `jvm-clj-ensure-jars' downloads: Clojure, its deps, and nREPL.
Each entry is a plist with :group, :artifact, :version, :repository (a
Maven repository URL), and :sha256 (of the jar).  To use other versions,
change the entries; a jar whose sha256 differs is rejected.")

(defvar jvm-clj-maven-local-repo "~/.m2/repository"
  "Local Maven repository where `jvm-clj-ensure-jars' caches jars.
The default is the one that Maven and Clojure CLI (`clj') use, so jars
are shared with them.")

(defvar jvm-clj--source-dir
  (expand-file-name "clojure/"
                    (file-name-directory
                     (or load-file-name buffer-file-name)))
  "Absolute path to this package's `clojure' directory.
A source checkout sets it in scripts/dev-paths.el.")
```

Before the `;;; Startup.` section add:

```elisp
;;; Dependencies.

(defun jvm-clj--jar-relative-path (dependency)
  "Return the Maven repository path of DEPENDENCY's jar."
  (let ((group (plist-get dependency :group))
        (artifact (plist-get dependency :artifact))
        (version (plist-get dependency :version)))
    (format "%s/%s/%s/%s-%s.jar"
            (subst-char-in-string ?. ?/ group) artifact version artifact version)))

(defun jvm-clj--check-sha256 (file expected)
  "Signal an error unless FILE's sha256 is EXPECTED."
  (let ((actual (with-temp-buffer
                  (set-buffer-multibyte nil)
                  (insert-file-contents-literally file)
                  (secure-hash 'sha256 (current-buffer)))))
    (unless (equal actual expected)
      (error "jvm-clj: %s has sha256 %s, expected %s" file actual expected))))

(defun jvm-clj--ensure-jar (dependency)
  "Return the local path of DEPENDENCY's jar.  Download it if missing.
Check the sha256 of a cached jar and of a downloaded one.  A download
goes to a temporary file first, so a failed or corrupt download does
not stay in the cache."
  (let* ((relative (jvm-clj--jar-relative-path dependency))
         (local (expand-file-name relative jvm-clj-maven-local-repo))
         (expected (plist-get dependency :sha256)))
    (if (file-exists-p local)
        (jvm-clj--check-sha256 local expected)
      (let ((url (concat (plist-get dependency :repository) "/" relative))
            (directory (file-name-directory local)))
        (make-directory directory t)
        (let ((temp (make-temp-file (expand-file-name ".download-" directory))))
          (unwind-protect
              (progn
                (message "jvm-clj: downloading %s" url)
                (url-copy-file url temp t)
                (jvm-clj--check-sha256 temp expected)
                (rename-file temp local))
            (when (file-exists-p temp)
              (delete-file temp))))))
    local))

(defun jvm-clj-ensure-jars ()
  "Return the local paths of the jars in `jvm-clj-dependencies'.
Download a missing jar into `jvm-clj-maven-local-repo'.  This blocks
Emacs.  Signal an error if a jar's sha256 does not match."
  (mapcar #'jvm-clj--ensure-jar jvm-clj-dependencies))

(defun jvm-clj--classpath ()
  "Return the classpath entries that `jvm-clj-start' loads."
  (append (or jvm-clj-classpath (jvm-clj-ensure-jars))
          (list jvm-clj--source-dir)))
```

In `jvm-clj-start-await` and `jvm-clj-start-direct`, replace
`(append jvm-clj-classpath (list jvm-clj--source-dir))` with `(jvm-clj--classpath)`.

- [ ] **Step 5: Run the new tests**

Run: `mise run test test/jvm-clj-jars-test.el` → 4 pass.

- [ ] **Step 6: Convert the Clojure tests**

In `test/jvm-clj-test.el` and `test/jvm-clj-direct-test.el`: delete the `--jars-dir` defvar, the
skip function/`ert-skip` form, and the `(setq jvm-clj-classpath ...)` line. The `ensure-ready`
function keeps only its start logic (`jvm-clj-start-await` uses `jvm-clj-ensure-jars`).

In `test/jvm-clj-eval-test.el`, `test/jvm-clj-eval-direct-test.el`,
`test/jvm-symbol-marshal-test.el`: delete the `--jars-dir` defvar and skip forms; replace
`(directory-files ...--jars-dir t "\\.jar\\'")` with `(jvm-clj-ensure-jars)`. Fix any docstring
that mentions `EMACS_JNI_CLOJURE_JARS_DIR` or the fetch script.

Delete `test/fetch-clojure-jars.sh`.

- [ ] **Step 7: Run the Clojure tests**

Run: `mise run test test/jvm-clj-test.el test/jvm-clj-direct-test.el test/jvm-clj-eval-test.el test/jvm-clj-eval-direct-test.el test/jvm-symbol-marshal-test.el`
Expected: all pass, none skipped. `ls ~/.m2/repository/org/clojure/clojure/1.12.0/` shows the jar.

- [ ] **Step 8: Docs**

- `lisp-api.md`: add `jvm-clj-ensure-jars` `()` → list of jar paths to the `jvm-clj.el`
  functions table. Variables table: update `jvm-clj-classpath` ("If `nil`, `jvm-clj-start` uses
  `jvm-clj-ensure-jars`."), add `jvm-clj-dependencies` and `jvm-clj-maven-local-repo` rows.
- `configuration.md`: delete the `EMACS_JNI_CLOJURE_JARS_DIR` row and the
  `target/clojure-jars/` row. "Prerequisites for Clojure": "`jvm-clj-start` downloads these jars
  into `jvm-clj-maven-local-repo` (default `~/.m2/repository`) and checks their sha256. Set
  `jvm-clj-classpath` to use other jars."
- `implementation.md:215`: delete the `EMACS_JNI_CLOJURE_JARS_DIR` row.
- `access-lisp-from-clojure.md:15`: delete the `setq jvm-clj-classpath` form; `(jvm-clj-start)`
  is enough.
- README Testing: replace the Clojure-jar paragraph with "The Clojure tests download the Clojure
  and nREPL jars into `~/.m2/repository` on the first run."

- [ ] **Step 9: Commit**

```bash
git add -A elisp/jvm-clj.el test docs README.md
git commit -m "feat(clojure): Download and cache Clojure jars in jvm-clj"
```

### Task 4: Package archive

**Files:**
- Create: `scripts/package.sh`, `scripts/package-test.el`
- Modify: `.config/mise/config.toml`, `docs/reference/configuration.md` (Build outputs)

**Interfaces:**
- Consumes: `jvm-module-directory` default and `jvm-clj--source-dir` default (Tasks 1, 3).
- Produces: `target/package/jvm-<version>.tar`; `mise run package`, `mise run package-test`.

- [ ] **Step 1: Create `scripts/package.sh`** (executable)

```bash
#!/usr/bin/env bash
# Build target/package/jvm-<version>.tar, a package.el archive for the host platform. Install it
# with M-x package-install-file.
set -euo pipefail

cd "$(dirname "$0")/.."

cargo build --release

PKGID="$(cargo pkgid)"
VERSION="${PKGID##*[#@]}"
SUFFIX="$(emacs -Q --batch --eval '(princ module-file-suffix)')"
case "$SUFFIX" in
    .dll) MODULE="emacs_jni$SUFFIX" ;;
    *) MODULE="libemacs_jni$SUFFIX" ;;
esac

OUT_DIR=target/package
PKG_DIR="$OUT_DIR/jvm-$VERSION"
rm -rf "$PKG_DIR"
rm -f "$OUT_DIR"/jvm-*.tar
mkdir -p "$PKG_DIR"
cp elisp/jvm.el elisp/jvm-clj.el "target/release/$MODULE" "$PKG_DIR/"
cp -r clojure "$PKG_DIR/"
cat > "$PKG_DIR/jvm-pkg.el" <<EOF
(define-package "jvm" "$VERSION"
  "Embed a JVM in Emacs; call Java and Clojure from Lisp"
  '((emacs "28.1")))
EOF
tar -cf "$OUT_DIR/jvm-$VERSION.tar" -C "$OUT_DIR" "jvm-$VERSION"
echo "$OUT_DIR/jvm-$VERSION.tar"
```

On macOS cargo emits `libemacs_jni.dylib`, which matches `module-file-suffix` `.dylib` (Emacs
28+). On Windows it emits `emacs_jni.dll`.

- [ ] **Step 2: Create `scripts/package-test.el`**

```elisp
;;; package-test.el --- Install the jvm archive and smoke-test it -*- lexical-binding: t; -*-

;;; Commentary:

;; Usage: emacs -Q --batch -l scripts/package-test.el TAR
;;
;; Install TAR into a scratch `package-user-dir', then start the JVM, make
;; one Java call, and evaluate one Clojure form.  Exit non-zero on failure.

;;; Code:

(require 'package)

(let* ((tar (expand-file-name (car command-line-args-left)))
       (root (file-name-directory (directory-file-name (file-name-directory tar)))))
  (setq package-user-dir (expand-file-name "package-test-elpa" root))
  (when (file-directory-p package-user-dir)
    (delete-directory package-user-dir t))
  (package-initialize)
  (package-install-file tar)
  (require 'jvm-clj)
  (unless (eq (jvm-start) 'ready)
    (error "JVM status: %s" (jvm-status)))
  (unless (equal (jvm-call "java.lang.Integer" "toHexString" '(255)) "ff")
    (error "jvm-call returned a wrong value"))
  (jvm-clj-start)
  (unless (equal (jvm-clj-eval-string "(+ 1 2)") 3)
    (error "jvm-clj-eval-string returned a wrong value"))
  (message "package-test: OK"))

;;; package-test.el ends here
```

`root` is `target/` (the tar is in `target/package/`).

- [ ] **Step 3: Add tasks**

```toml
[tasks.package]
description = "Build the package.el archive target/package/jvm-<version>.tar"
run = "scripts/package.sh"

[tasks.package-test]
description = "Install the archive in a scratch package dir and smoke-test it"
depends = ["package"]
run = "emacs -Q --batch -l scripts/package-test.el target/package/jvm-*.tar"
```

The glob matches one file because `package.sh` removes old archives (Step 1).

- [ ] **Step 4: Verify**

Run: `mise run package-test`
Expected: `package-test: OK`. `tar -tf target/package/jvm-0.1.0.tar` lists `jvm-0.1.0/jvm.el`,
`jvm-clj.el`, `jvm-pkg.el`, `libemacs_jni.so`, `clojure/emacs_jni/clj.clj`.

- [ ] **Step 5: Docs**

`configuration.md` Build outputs: add rows for `target/release/libemacs_jni.so` and
`target/package/jvm-<version>.tar` (`mise run package`); `target/package-test-elpa/`
(`mise run package-test`). Change the `target/test-fixtures/` producer to `mise run fixtures`.

- [ ] **Step 6: Commit**

```bash
git add scripts .config/mise/config.toml docs/reference/configuration.md
git commit -m "feat(package): Add package.el archive and smoke test tasks"
```

### Task 5: README and tutorials

**Files:**
- Modify: `README.md`, `docs/tutorials/01-your-first-jvm-call.md`,
  `docs/tutorials/02-evaluate-clojure-and-connect-nrepl.md`, `docs/reference/configuration.md`
  (Prerequisites)

- [ ] **Step 1: README**

Replace "Build" and "Quick taste" with:

- "Install": `mise run package`, then `M-x package-install-file RET target/package/jvm-0.1.0.tar`.
  Then `(require 'jvm)`. The archive holds a module for the host platform only.
- "Develop": `mise run build`, then `emacs -Q -l scripts/dev-paths.el`. The quick-taste forms
  follow unchanged.

Prerequisites: "Linux. Other platforms are not tested." Add `mise` as the task runner. Same
change in `configuration.md` Prerequisites ("Operating system").

- [ ] **Step 2: Tutorial 1**

Step 1 block becomes `mise run fixtures`; the text says it builds the module and
`target/test-fixtures/{greeter,calc}.jar`. Step 2 command becomes `emacs -Q -l scripts/dev-paths.el`
and its explanation says the file puts `elisp/` on `load-path` and points `jvm` at
`target/debug/`. Step 5: delete the `javac`/`jar` block and the shim-classes paragraph; keep
"Load the new jar".

- [ ] **Step 3: Tutorial 2**

Delete Step 1 (fetch jars) and the env var. New Step 1: `mise run build`, then
`emacs -Q -l scripts/dev-paths.el`. In the old Step 3, delete the `setq jvm-clj-classpath` form;
say that the first `jvm-clj-start` downloads four jars into `~/.m2/repository` and takes some
seconds. Renumber steps.

- [ ] **Step 4: Check no stale references remain**

Run: `rg -n "EMACS_JNI_MODULE_PATH|EMACS_JNI_CLOJURE_JARS_DIR|fetch-clojure-jars|jvm--module-path|-L elisp" --glob '!docs/superpowers/**' --glob '!.superpowers/**' --glob '!scratch.el' --glob '!*.html'`
Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add README.md docs
git commit -m "docs: Use mise tasks and the package archive in README and tutorials"
```

### Task 6: Full verification

- [ ] **Step 1:** `mise run test` → Rust unit tests and all ERT files pass; nothing skipped
  except `jvm-crash-test.el`.
- [ ] **Step 2:** `mise run package-test` → `package-test: OK`.
- [ ] **Step 3:** `cargo clippy` and `cargo fmt --check` → clean (no Rust change expected).
