#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

# jni-rs's `invocation` feature locates libjvm.so via JAVA_HOME (or `java`
# on PATH) at runtime. Export it here so tests that create a real JVM don't
# depend on every invoking shell already having it set.
if [ -z "${JAVA_HOME:-}" ] && command -v mise >/dev/null 2>&1; then
    export JAVA_HOME="$(mise where java)"
fi

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
