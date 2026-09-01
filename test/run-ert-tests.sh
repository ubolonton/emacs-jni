#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

# jni-rs's `invocation` feature locates libjvm.so via JAVA_HOME (or `java`
# on PATH) at runtime. Export it here so tests that create a real JVM don't
# depend on every invoking shell already having it set.
if [ -z "${JAVA_HOME:-}" ] && command -v mise >/dev/null 2>&1; then
    export JAVA_HOME="$(mise where java)"
fi

mkdir -p target/test-fixtures
javac -d target/test-fixtures test/fixtures/Greeter.java
jar cf target/test-fixtures/greeter.jar -C target/test-fixtures Greeter.class
export EMACS_JNI_GREETER_JAR="$(pwd)/target/test-fixtures/greeter.jar"

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
    # test/jvm-crash-test.el calls `jvm--test-panic', which only exists when
    # built with `--features test-support'. Without that feature the default
    # build lacks the defun, so auto-discovery would always fail on this one
    # file with an opaque void-function error; skip it here instead, with a
    # note on how to run it explicitly.
    if ! [[ "$CARGO_FEATURES" =~ (^|[,[:space:]])test-support($|[,[:space:]]) ]]; then
        FILTERED_TEST_FILES=()
        for f in "${TEST_FILES[@]}"; do
            if [ "$f" = "test/jvm-crash-test.el" ]; then
                echo "Skipping test/jvm-crash-test.el (needs --features test-support); run: EMACS_JNI_TEST_FEATURES=test-support ./test/run-ert-tests.sh test/jvm-crash-test.el"
            else
                FILTERED_TEST_FILES+=("$f")
            fi
        done
        TEST_FILES=("${FILTERED_TEST_FILES[@]}")
    fi
fi

# Each test file gets its own `emacs --batch` process: the embedded JVM and
# Rust's STATUS static are both per-process singletons that persist for the
# life of one Emacs process, so test files that each expect a fresh
# not-started JVM would otherwise collide if loaded together into one
# process.
overall_status=0
for f in "${TEST_FILES[@]}"; do
    echo "=== $f ==="
    if ! emacs -Q --batch -L elisp -l "$f" -f ert-run-tests-batch-and-exit; then
        overall_status=1
    fi
done
exit "$overall_status"
