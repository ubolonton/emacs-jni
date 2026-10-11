# Packaging design

## Goal

An installable package.el archive, mise tasks for build/test/package, and shorter tutorials.

## Module lookup (`elisp/jvm.el`)

- New public `jvm-module-directory`. Default: the directory of `jvm.el` (installed-package
  layout). Set it before `(require 'jvm)` to use another build, e.g. `target/debug`.
- `jvm.el` loads `<jvm-module-directory>/<cargo file name>`: `lib` prefix except on Windows,
  `emacs_jni`, then `module-file-suffix` (`.so`, `.dylib`, `.dll`). No search over several paths.
- `jvm--module-path` and `EMACS_JNI_MODULE_PATH` are removed.
- `jvm--source-directory` (private) holds `src/`, `java/`, `clojure/`. Its default is the
  directory of `jvm.el` (installed-package layout). `jvm-clj--classpath`
  adds its `clojure/`.
- `dev-paths.el` (repo root) sets `load-path`, `jvm-module-directory` (`target/debug/`), and
  `jvm--source-directory` (the root) for a checkout: `emacs -Q -l dev-paths.el`.
  Tests and docs use it.

## mise tasks (`.config/mise/config.toml`)

Every helper script is in `.config/scripts/`, declared in `config.toml` with `file =` (the same
convention as the doc-lint tasks). Short tasks are inline `run =`.

| Task | Does |
|---|---|
| `build` | `cargo build` (debug). |
| `fixtures` | Builds `target/test-fixtures/{greeter,calc}.jar`. Depends on `build` (Calc needs the shim classes). |
| `test` | Dev setup (`dev-paths.el`). No args: `cargo test --lib`, then all ERT files. With file args: only those ERT files. Honors `EMACS_JNI_TEST_FEATURES`. |
| `test:package` | Depends on `package`. Installs the tar into `target/package-test-elpa/`, then runs the same ERT files against it through package.el. Skips the crash test (no `test-support`). |
| `package` | `cargo build --release`, then makes `target/package/jvm-<version>.tar`. |

Archive layout, `jvm-<version>/`:

- `jvm.el`, `jvm-clj.el`
- `jvm-pkg.el`, generated. Version from `Cargo.toml`.
- the module, with cargo's file name. The suffix comes from batch Emacs `module-file-suffix`.
- `clojure/`, loaded by `jvm-clj-start`
- `src/`, `java/`, for source browsing. All three source directories keep their checkout paths.

The archive targets the host platform only.

## Clojure jars (`elisp/jvm-clj.el`)

- `jvm-clj-dependencies`: a public list of plists, `:group :artifact :version :repository
  :sha256`. It pins Clojure 1.12.0, spec.alpha 0.5.238, core.specs.alpha 0.4.74, and nREPL 1.7.0.
  It is the only copy of this list.
- `jvm-clj-maven-local-repo`: default `~/.m2/repository`. This is the location and layout that
  `clj`/tools.deps and Maven use, so jars are shared with them.
- `jvm-clj-ensure-jars`: for each dependency, downloads a missing jar (temp file, then rename),
  checks the sha256 of each jar, and signals an error on a mismatch. Returns the jar paths. The
  first download blocks Emacs and shows a message.
- If `jvm-clj-classpath` is nil, `jvm-clj-start` uses `(jvm-clj-ensure-jars)`. If the user sets
  `jvm-clj-classpath`, no download occurs.
- `test/fetch-clojure-jars.sh` and `EMACS_JNI_CLOJURE_JARS_DIR` are removed. Clojure tests do not
  skip; they use `jvm-clj-ensure-jars`. The first run needs the network.

## Docs

- README: "Install" (`mise run package`, `M-x package-install-file`) and "Develop"
  (`mise run build`, `jvm-module-directory`, `mise run test`).
- Tutorial 1: Step 1 is `mise run fixtures`. The `javac` commands in Step 5 are removed.
- Tutorial 2: no jar-fetch step and no env var. `(jvm-clj-start)` downloads the jars.
- `configuration.md`, `lisp-api.md`: new variables and function, removed env vars, build outputs,
  and the platform statement "tested on Linux".
