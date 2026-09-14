# Configuration reference

## Environment variables

| Variable | Read by | Effect |
|---|---|---|
| `EMACS_JNI_MODULE_PATH` | `elisp/jvm.el`, at `require` time. | Overrides the path to the built `.so` module. Without it, the path defaults to a location relative to `elisp/jvm.el` itself (`../target/debug/libemacs_jni.so`). |
| `EMACS_JNI_CLOJURE_JARS_DIR` | `test/run-ert-tests.sh`, `test/jvm-clj-eval-test.el`, and `test/fetch-clojure-jars.sh`'s printed usage line. Not read by the module itself. | Points the test suite at a directory of Clojure/nREPL jars. Application code that wants the same convention reads it with `getenv`, as the tutorials do — the module does not read it automatically. |
| `EMACS_JNI_TEST_FEATURES` | `test/run-ert-tests.sh`. | Set to `test-support` to build the module with its `test-support` Cargo feature, which enables the crash-recovery test in `test/jvm-crash-test.el`. |
| `JAVA_HOME` | The `jni` crate's `invocation` feature, at runtime. | Locates `libjvm.so`. If unset, `jni` falls back to finding `java` on `PATH`. |

## Build outputs

| Path | Produced by | Contents |
|---|---|---|
| `target/debug/libemacs_jni.so` | `cargo build` | The dynamic module `elisp/jvm.el` loads. |
| `target/emacs-jni-shim-classes/` | `cargo build`, through `build.rs` | Compiled Java shim classes, under `emacs_jni.core` — `LispCallable`, `LispFunction`, `LispNonLocalExit`, `ReleaseNotifier`, and (for Clojure Lisp access) `LispObject`, `Lisp`, `LispRefReleaseNotifier`, `LispSymbol`. Java code needs these on its classpath to reference the types directly, as `test/fixtures/Calc.java` does. |
| `target/clojure-jars/` | `test/fetch-clojure-jars.sh` | The four jars listed under [Prerequisites for Clojure](#prerequisites-for-clojure) below. Gitignored, never bundled or committed. |

## Configuration variables

`jvm-classpath`, `jvm-options`, `jvm-status-hook`, and (from
`jvm-clj.el`) `jvm-clj-classpath` are documented in the
[Lisp API reference](lisp-api.md#variables).

`jvm-clj-start` also loads this package's `clojure-src/` source
directory, which holds the `emacs-jni.clj` namespace, onto the
JVM classpath. It is part of the repository, not a build output.

## Prerequisites

| Requirement | Detail |
|---|---|
| Emacs | 28 or later. This module needs `Env::open_channel`, added in Emacs 28. Exercised against Emacs 30.1. |
| Rust | A stable toolchain. `cargo build` uses whatever is on `PATH`. |
| A JDK | Located via `JAVA_HOME`, or `java` on `PATH`, at runtime. This repository pins JDK 26 through [mise](https://mise.jdx.dev/) (`mise.toml`/`mise.lock`). Both files are currently untracked in this repository's git history — nobody has decided whether to commit them, so check with the maintainer before adding them. |
| Operating system | Linux only. macOS and Windows are out of scope. |
| `jni` crate version | Pinned to 0.22.4 in `Cargo.toml`. |

## Prerequisites for Clojure

Fetched by `test/fetch-clojure-jars.sh`, from the sources below. Not
bundled with this repository.

| Jar | Version | Source |
|---|---|---|
| `clojure-1.12.0.jar` | 1.12.0 | Maven Central |
| `spec.alpha-0.5.238.jar` | 0.5.238 | Maven Central |
| `core.specs.alpha-0.4.74.jar` | 0.4.74 | Maven Central |
| `nrepl-1.7.0.jar` | 1.7.0 | Clojars (not published to Maven Central) |
