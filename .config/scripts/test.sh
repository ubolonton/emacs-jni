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

# Absolute paths for Emacs. Native Windows Emacs does not understand MSYS2 paths like /c/foo, so
# convert them to C:/foo.
ROOT="$PWD"
if command -v cygpath >/dev/null; then
    ROOT="$(cygpath -m "$PWD")"
fi
if [ "$MODE" = dev ]; then
    # The fixtures need the shim classes that build.rs compiles.
    cargo build ${CARGO_FEATURES:+--features "$CARGO_FEATURES"}
    if [ $# -eq 0 ]; then
        cargo test --lib
    fi
    EMACS_SETUP=(-l dev-paths.el)
else
    ELPA_DIR="$ROOT/target/package-test-elpa"
    rm -rf "$ELPA_DIR"
    TAR="$(echo target/package/jvm-*.tar)"
    EMACS_SETUP=(--eval "(setq package-user-dir \"$ELPA_DIR\")" -f package-initialize)
    emacs -Q --batch "${EMACS_SETUP[@]}" --eval "(package-install-file \"$TAR\")"
fi

.config/scripts/build-fixtures.sh
export EMACS_JNI_GREETER_JAR="$ROOT/target/test-fixtures/greeter.jar"

if [ $# -eq 0 ]; then
    set -- test/*.el
fi

# One Emacs process per file: the JVM and the Rust status are per-process singletons.
overall_status=0
for f in "$@"; do
    echo "=== $f ==="
    if ! emacs -Q --batch "${EMACS_SETUP[@]}" -l "$f" -f ert-run-tests-batch-and-exit; then
        overall_status=1
    fi
done
exit "$overall_status"
