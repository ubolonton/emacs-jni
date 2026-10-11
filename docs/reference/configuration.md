# Configuration reference

## Environment variables

| Variable | Read by | Effect |
|---|---|---|
| `JAVA_HOME` | the module, when it creates the JVM | JDK used to find `libjvm.so`. If unset, the module uses the `java` executable on `PATH`. mise sets it in tasks. |
| `EMACS_JNI_TEST_FEATURES` | `mise run test` | Cargo features for the debug test build. `test-support` enables `test/jvm-crash-test.el`. Without it, the task skips that file. `mise run test:package` always skips it. |
| `EMACS_JNI_GREETER_JAR` | `test/jvm-load-jar-test.el` | Path of the test fixture jar. `mise run test` and `mise run test:package` set it. |

## Configuration variables

`jvm-classpath`, `jvm-options`, `jvm-status-hook`, `jvm-module-directory`, `jvm-clj-classpath`,
`jvm-clj-dependencies`, and `jvm-clj-maven-local-repo`:
see [Lisp API reference](lisp-api.md#variables).

## Build outputs

| Path | Produced by | Contents |
|---|---|---|
| `target/debug/libemacs_jni.so` | `mise run build` | The debug dynamic module. `dev-paths.el` points `jvm-module-directory` at it. |
| `target/release/libemacs_jni.so` | `mise run package` | The release dynamic module, copied into the archive. |
| `target/package/jvm-<version>.tar` | `mise run package` | The package.el archive for the host platform: `jvm.el`, `jvm-clj.el`, `jvm-pkg.el`, the module, `clojure/`. Install it with `M-x package-install-file`. |
| `target/package-test-elpa/` | `mise run test:package` | Scratch `package-user-dir` that the archive is installed into for the ERT tests. |
| `target/emacs-jni-shim-classes/` | `cargo build` (`build.rs` runs `javac`) | Compiled classes of package `emacs_jni.core`: `LispCallable`, `LispFunction`, `LispNonLocalExit`, `LispSignal`, `LispThrow`, `ReleaseNotifier`, `LispObject`, `Lisp`, `LispRefReleaseNotifier`, `LispSymbol`. Add this directory to the `javac` classpath to compile Java code that uses these types, as for `test/fixtures/Calc.java`. |
| `target/test-fixtures/` | `mise run fixtures`, `mise run test`, `mise run test:package` | `greeter.jar` and `calc.jar`, from `test/fixtures/`. |

`clojure/` holds the `emacs-jni.clj` namespace. It is source, not a build output.
`jvm-clj-start` adds it to the classpath.

## Prerequisites

| Requirement | Version |
|---|---|
| Operating system | Linux. macOS and Windows are not tested. `jvm.el` uses the module file name that cargo gives on each platform. |
| Emacs | 28 or later, with dynamic module support. Tested with 30.1. |
| Rust | Stable toolchain. |
| mise | Runs the tasks in `.config/mise/config.toml`; their scripts are in `.config/scripts/`. Installs the pinned JDK. |
| JDK | `javac` on `PATH` at build time. A JDK at run time (see `JAVA_HOME`). `.config/mise/config.toml` pins Java 26. `.config/mise/mise.lock` locks 26.0.2.1. |

## Prerequisites for Clojure

`jvm-clj-start` downloads these jars into `jvm-clj-maven-local-repo` (default `~/.m2/repository`)
and checks their sha256. `jvm-clj-dependencies` lists them. To use other jars, set
`jvm-clj-classpath`.

| Jar | Version | Source |
|---|---|---|
| `clojure-1.12.0.jar` | 1.12.0 | Maven Central |
| `spec.alpha-0.5.238.jar` | 0.5.238 | Maven Central |
| `core.specs.alpha-0.4.74.jar` | 0.4.74 | Maven Central |
| `nrepl-1.7.0.jar` | 1.7.0 | Clojars |
