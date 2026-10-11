# Configuration reference

## Environment variables

| Variable | Read by | Effect |
|---|---|---|
| `JAVA_HOME` | the module, when it creates the JVM | JDK used to find `libjvm.so`. If unset, the module uses the `java` executable on `PATH`. `test/run-ert-tests.sh` sets it from `mise where java` if it is unset and `mise` is available. |
| `EMACS_JNI_CLOJURE_JARS_DIR` | the Clojure tests (`test/jvm-clj-*.el`, `test/jvm-symbol-marshal-test.el`) | Directory of Clojure and nREPL jars. If unset, these tests skip. The module and `jvm-clj.el` do not read it. The tutorials and how-to guides read it with `getenv`. |
| `EMACS_JNI_TEST_FEATURES` | `test/run-ert-tests.sh` | Cargo features for the test build. `test-support` enables `test/jvm-crash-test.el`. Without it, the script skips that file. |
| `EMACS_JNI_GREETER_JAR` | `test/jvm-load-jar-test.el` | Path of the test fixture jar. `test/run-ert-tests.sh` sets it. |

## Configuration variables

`jvm-classpath`, `jvm-options`, `jvm-status-hook`, `jvm-module-directory`, and `jvm-clj-classpath`:
see [Lisp API reference](lisp-api.md#variables).

## Build outputs

| Path | Produced by | Contents |
|---|---|---|
| `target/debug/libemacs_jni.so` | `cargo build` | The dynamic module that `elisp/jvm.el` loads. |
| `target/emacs-jni-shim-classes/` | `cargo build` (`build.rs` runs `javac`) | Compiled classes of package `emacs_jni.core`: `LispCallable`, `LispFunction`, `LispNonLocalExit`, `LispSignal`, `LispThrow`, `ReleaseNotifier`, `LispObject`, `Lisp`, `LispRefReleaseNotifier`, `LispSymbol`. Add this directory to the `javac` classpath to compile Java code that uses these types, as for `test/fixtures/Calc.java`. |
| `target/test-fixtures/` | `test/run-ert-tests.sh` | `greeter.jar` and `calc.jar`, from `test/fixtures/`. |
| `target/clojure-jars/` | `test/fetch-clojure-jars.sh` | The jars in [Prerequisites for Clojure](#prerequisites-for-clojure). The script takes an optional output directory argument. |

`clojure-src/` holds the `emacs-jni.clj` namespace. It is source, not a build output.
`jvm-clj-start` adds it to the classpath.

## Prerequisites

| Requirement | Version |
|---|---|
| Operating system | Linux. macOS and Windows are not supported. |
| Emacs | 28 or later, with dynamic module support. Tested with 30.1. |
| Rust | Stable toolchain. |
| JDK | `javac` on `PATH` at build time. A JDK at run time (see `JAVA_HOME`). `.config/mise/config.toml` pins Java 26. `.config/mise/mise.lock` locks 26.0.2.1. |

## Prerequisites for Clojure

`test/fetch-clojure-jars.sh` downloads these jars. The repository does not include them.

| Jar | Version | Source |
|---|---|---|
| `clojure-1.12.0.jar` | 1.12.0 | Maven Central |
| `spec.alpha-0.5.238.jar` | 0.5.238 | Maven Central |
| `core.specs.alpha-0.4.74.jar` | 0.4.74 | Maven Central |
| `nrepl-1.7.0.jar` | 1.7.0 | Clojars |
