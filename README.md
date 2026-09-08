# emacs-jni

An Emacs dynamic module (Rust, using the [`emacs`](https://github.com/ubolonton/emacs-module-rs)
crate) that embeds a JVM via [`jni`](https://github.com/jni-rs/jni-rs).
It gives Emacs Lisp control over a JVM's lifecycle, two-way calls
between Lisp and Java, Clojure evaluation, a real nREPL server, and
Clojure access to Emacs Lisp values.

**Status: done.** All three planned phases (JVM lifecycle, JVM ↔ Lisp
interop, Clojure/nREPL) have shipped, plus a follow-on that lets
Clojure hold and call Emacs Lisp values (`jvm-clj.el` /
`dev.ublt.emacs.clj`).

**Full documentation, in tutorial, how-to, reference, and explanation
form:** [`docs/index.md`](docs/index.md). Start there for anything
beyond building this repository.

Design rationale from development itself:
[`docs/superpowers/specs/2026-08-31-jvm-lifecycle-design.md`](docs/superpowers/specs/2026-08-31-jvm-lifecycle-design.md)
(Phase 1),
[`docs/superpowers/specs/2026-09-01-jvm-lisp-interop-design.md`](docs/superpowers/specs/2026-09-01-jvm-lisp-interop-design.md)
(Phase 2), and
[`docs/superpowers/specs/2026-09-02-clojure-nrepl-design.md`](docs/superpowers/specs/2026-09-02-clojure-nrepl-design.md)
(Phase 3), and
[`docs/superpowers/specs/2026-09-08-clojure-lisp-access-design.md`](docs/superpowers/specs/2026-09-08-clojure-lisp-access-design.md)
(Clojure Lisp access).

## Prerequisites

- Emacs 28+ (exercised against Emacs 30.1). Needs `Env::open_channel`,
  added in Emacs 28.
- Rust (stable toolchain). `cargo build` picks up whatever's on `PATH`.
- A JDK. This repo pins JDK 26 via [mise](https://mise.jdx.dev/).
  `jni`'s `invocation` feature locates `libjvm.so` via `JAVA_HOME` (or
  `java` on `PATH`) at runtime, so make sure one of those points at
  your JDK.
- Linux only for now (macOS/Windows are out of scope).

Full detail, including environment variables and build outputs:
[`docs/reference/configuration.md`](docs/reference/configuration.md).

## Build

```sh
cd emacs-jni
cargo build
```

This produces `target/debug/libemacs_jni.so`, which `elisp/emacs-jni.el`
loads automatically.

## Try it

[`docs/tutorials/`](docs/tutorials/) walks through starting the JVM,
calling Java from Lisp, evaluating Clojure, and running an nREPL
server, with commands you can copy and paste. For a specific task
instead of a guided walkthrough, see
[`docs/how-to-guides/`](docs/how-to-guides/). For the full Lisp API,
see [`docs/reference/lisp-api.md`](docs/reference/lisp-api.md). For
why it's built this way, and every known limitation, see
[`docs/explanation/`](docs/explanation/).

## Testing

```sh
cargo test --lib -- --test-threads=1       # pure-Rust unit tests, no Emacs/JVM
./test/run-ert-tests.sh                    # full integration suite, real embedded JVM
EMACS_JNI_TEST_FEATURES=test-support ./test/run-ert-tests.sh   # + crash-recovery test
```

`--test-threads=1` matters for `cargo test --lib`: its default
parallel mode is flaky, from a `jni`-rs race across concurrent
first-time `JavaVM::new()` calls in different test modules. See
[`docs/explanation/limitations-and-tradeoffs.md`](docs/explanation/limitations-and-tradeoffs.md#a-restart-after-a-crash-or-stop-does-not-fully-recreate-the-jvm).

Each `test/*.el` file runs in its own `emacs --batch` process (the
embedded JVM is a per-process singleton, so test files can't safely
share one).

The Clojure tests in `test/jvm-clojure-test.el` and
`test/jvm-clj-test.el` (Lisp access) are gated behind
`EMACS_JNI_CLOJURE_JARS_DIR` — they skip cleanly, not fail, when it's
unset, so the rest of the suite stays runnable with no network access:

```sh
./test/fetch-clojure-jars.sh
EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" EMACS_JNI_TEST_FEATURES=test-support ./test/run-ert-tests.sh
```

## What's next

The original three-phase scope is complete. There is no mandated next
phase. Possibilities worth considering, if this project gets picked up
again — not a plan, not scoped, not designed:

- **Packaging and distribution.** Today this only runs from a source
  checkout (`cargo build` plus `-L elisp`). No MELPA recipe, no
  prebuilt binaries, no bundled JDK or Clojure — deliberately, per
  this project's "point at your own JDK/jars" policy.
- **Richer Clojure value marshalling.** Teaching `jvm-call`'s
  marshalling about Clojure keywords, vectors, and maps directly,
  instead of the generic opaque-handle fallback, would remove the
  biggest practical friction in `jvm-clojure-eval-string` — at the
  cost of picking a Lisp-side representation for each, a real design
  question rather than a bounded addition.
- Addressing any item in
  [`docs/explanation/limitations-and-tradeoffs.md`](docs/explanation/limitations-and-tradeoffs.md),
  if a real need for one surfaces.
