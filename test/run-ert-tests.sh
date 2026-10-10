#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

# jni-rs finds libjvm.so through JAVA_HOME.
if [ -z "${JAVA_HOME:-}" ] && command -v mise >/dev/null 2>&1; then
    export JAVA_HOME="$(mise where java)"
fi

mkdir -p target/test-fixtures
javac -d target/test-fixtures test/fixtures/Greeter.java
jar cf target/test-fixtures/greeter.jar -C target/test-fixtures Greeter.class
export EMACS_JNI_GREETER_JAR="$(pwd)/target/test-fixtures/greeter.jar"

# Build first: Calc.java uses the shim classes that build.rs compiles into
# target/emacs-jni-shim-classes.
CARGO_FEATURES="${EMACS_JNI_TEST_FEATURES:-}"
if [ -n "$CARGO_FEATURES" ]; then
    cargo build --features "$CARGO_FEATURES"
else
    cargo build
fi

javac -cp target/emacs-jni-shim-classes -d target/test-fixtures test/fixtures/Calc.java
jar cf target/test-fixtures/calc.jar -C target/test-fixtures Calc.class

export EMACS_JNI_MODULE_PATH="$(pwd)/target/debug/libemacs_jni.so"

TEST_FILES=("$@")
if [ ${#TEST_FILES[@]} -eq 0 ]; then
    TEST_FILES=(test/*.el)
    # jvm-crash-test.el needs `jvm--test-panic', from `--features test-support'.
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

# One Emacs process per file: the JVM and the Rust status are per-process
# singletons.
overall_status=0
for f in "${TEST_FILES[@]}"; do
    echo "=== $f ==="
    if ! emacs -Q --batch -L elisp -l "$f" -f ert-run-tests-batch-and-exit; then
        overall_status=1
    fi
done
exit "$overall_status"
