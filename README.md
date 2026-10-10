# emacs-jni

An Emacs dynamic module that embeds a JVM in the Emacs process. Emacs Lisp starts and pauses the
JVM, loads jars, and calls Java methods. Java calls Lisp functions back. Clojure code evaluates
in the same JVM, reads Lisp values, and can serve an nREPL client. The module is written in Rust,
with the [`emacs`](https://github.com/ubolonton/emacs-module-rs) and
[`jni`](https://github.com/jni-rs/jni-rs) crates.

Documentation: [`docs/index.md`](docs/index.md).

## Prerequisites

- Linux.
- Emacs 28 or later.
- A stable Rust toolchain.
- A JDK, found through `JAVA_HOME` or `java` on `PATH`. The repository pins JDK 26 with
  [mise](https://mise.jdx.dev/).

Details: [Configuration reference](docs/reference/configuration.md).

## Build

```sh
cargo build
```

This makes `target/debug/libemacs_jni.so`. `elisp/jvm.el` loads it.

## Quick taste

Start `emacs -Q -L elisp` from the repository root, then evaluate:

```elisp
(require 'jvm)
(jvm-start)                                                ;; => ready
(jvm-call "java.lang.Integer" "toHexString" '(255))        ;; => "ff"
(jvm-call "java.lang.System" "getProperty" '("os.name"))   ;; => "Linux"
(jvm-call (jvm-new "java.util.ArrayList" nil) "size" nil)  ;; => 0
```

For a guided start, see [Your first JVM call](docs/tutorials/01-your-first-jvm-call.md).

## Testing

```sh
cargo test --lib                # Rust unit tests
./test/run-ert-tests.sh         # ERT integration tests, one emacs --batch process per file
./test/run-ert-tests.sh test/jvm-call-test.el                    # one test file
EMACS_JNI_TEST_FEATURES=test-support ./test/run-ert-tests.sh     # adds the crash test
```

`run-ert-tests.sh` builds the module and the test fixture jars first. The Clojure tests skip
unless `EMACS_JNI_CLOJURE_JARS_DIR` names a directory with the Clojure and nREPL jars:

```sh
./test/fetch-clojure-jars.sh    # downloads to target/clojure-jars/
EMACS_JNI_CLOJURE_JARS_DIR="$PWD/target/clojure-jars" ./test/run-ert-tests.sh
```

For how the Rust side is built, see [Implementation](docs/explanation/implementation.md).
