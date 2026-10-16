# emacs-jni

An Emacs dynamic module that embeds a JVM in the Emacs process. Emacs Lisp starts and pauses the
JVM, loads jars, and calls Java methods. Java calls Lisp functions back. Clojure code evaluates
in the same JVM, reads Lisp values, and can serve an nREPL client. The module is written in Rust,
with the [`emacs`](https://github.com/ubolonton/emacs-module-rs) and
[`jni`](https://github.com/jni-rs/jni-rs) crates.

Documentation: [`docs/index.md`](docs/index.md).

## Prerequisites

- Linux, macOS, or Windows (MSYS2; see the configuration reference).
- Emacs 28 or later.
- A stable Rust toolchain.
- A JDK, found through `JAVA_HOME` or `java` on `PATH`.
- [mise](https://mise.jdx.dev/). It pins JDK 26 and defines the tasks below.

Details: [Configuration reference](docs/reference/configuration.md).

## Install

```sh
mise run package    # makes target/package/jvm-0.1.0.tar
```

In Emacs, `M-x package-install-file RET target/package/jvm-0.1.0.tar`. The archive holds the module
for the host platform only.

## Develop

```sh
mise run build      # makes the module in target/debug/
emacs -Q -l dev-paths.el
```

`dev-paths.el` puts `elisp/` on `load-path` and points `jvm` at `target/debug/`. In Emacs, evaluate:

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
mise run test                                          # Rust unit tests, then every ERT file
mise run test tests/elisp/jvm-call-test.el             # only the given ERT files
EMACS_JNI_TEST_FEATURES=test-support mise run test tests/elisp/jvm-crash-test.el
mise run test:package                                  # every ERT file, against the installed archive
mise run lint                                          # rustfmt and clippy checks
```

Each ERT file runs in its own `emacs --batch` process. `test` loads the checkout through
`dev-paths.el`. `test:package` builds the archive, installs it into a scratch package directory,
and loads it through package.el. On the first run, the Clojure tests download the Clojure and
nREPL jars into `~/.m2/repository`.

CI (`.github/workflows/main.yml`) runs these tasks for pull requests and pushes to `main`, on Linux
(x86-64, arm64), macOS (arm64), and Windows, with Emacs 28 to 31.

For how the Rust side is built, see [Implementation](docs/explanation/implementation.md).
