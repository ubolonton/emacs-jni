#!/usr/bin/env bash
# Usage: test.sh [--package] [ERT-FILE...]
#
# Without --package, test the checkout: build the debug module (with the cargo features in
# EMACS_JNI_TEST_FEATURES) and load it through dev-paths.el. With no ERT-FILE, also run the Rust
# unit tests.
#
# With --package, test the archive that `mise run package` built: install it into a scratch
# `package-user-dir` and load it through package.el.
#
# Without ERT-FILE, run every test/*.el file.
set -euo pipefail

MODE=dev
if [ "${1:-}" = --package ]; then
    MODE=package
    shift
fi

CARGO_FEATURES="${EMACS_JNI_TEST_FEATURES:-}"
if [ "$MODE" = dev ]; then
    # The fixtures need the shim classes that build.rs compiles.
    if [ -n "$CARGO_FEATURES" ]; then
        cargo build --features "$CARGO_FEATURES"
    else
        cargo build
    fi
    if [ $# -eq 0 ]; then
        cargo test --lib
    fi
    EMACS_SETUP=(-l dev-paths.el)
else
    ELPA_DIR="$PWD/target/package-test-elpa"
    rm -rf "$ELPA_DIR"
    TAR="$(echo target/package/jvm-*.tar)"
    emacs -Q --batch \
          --eval "(setq package-user-dir \"$ELPA_DIR\")" \
          -f package-initialize \
          --eval "(package-install-file \"$TAR\")"
    EMACS_SETUP=(--eval "(setq package-user-dir \"$ELPA_DIR\")" -f package-initialize)
fi

.config/scripts/build-fixtures.sh
export EMACS_JNI_GREETER_JAR="$PWD/target/test-fixtures/greeter.jar"

TEST_FILES=("$@")
if [ ${#TEST_FILES[@]} -eq 0 ]; then
    TEST_FILES=(test/*.el)
    # jvm-crash-test.el needs `jvm--test-panic', which only a dev build with
    # `--features test-support' has.
    if [ "$MODE" = package ] \
           || ! [[ "$CARGO_FEATURES" =~ (^|[,[:space:]])test-support($|[,[:space:]]) ]]; then
        FILTERED_TEST_FILES=()
        for f in "${TEST_FILES[@]}"; do
            if [ "$f" = "test/jvm-crash-test.el" ]; then
                echo "Skipping test/jvm-crash-test.el (needs a dev build with test-support); run: EMACS_JNI_TEST_FEATURES=test-support mise run test test/jvm-crash-test.el"
            else
                FILTERED_TEST_FILES+=("$f")
            fi
        done
        TEST_FILES=("${FILTERED_TEST_FILES[@]}")
    fi
fi

# One Emacs process per file: the JVM and the Rust status are per-process singletons.
overall_status=0
for f in "${TEST_FILES[@]}"; do
    echo "=== $f ==="
    if ! emacs -Q --batch "${EMACS_SETUP[@]}" -l "$f" -f ert-run-tests-batch-and-exit; then
        overall_status=1
    fi
done
exit "$overall_status"
