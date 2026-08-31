#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

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
fi

LOAD_ARGS=()
for f in "${TEST_FILES[@]}"; do
    LOAD_ARGS+=(-l "$f")
done

emacs -Q --batch -L elisp "${LOAD_ARGS[@]}" -f ert-run-tests-batch-and-exit
