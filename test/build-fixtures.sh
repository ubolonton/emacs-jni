#!/usr/bin/env bash
# Build the test fixture jars into target/test-fixtures/. Needs the shim classes that `cargo build`
# puts in target/emacs-jni-shim-classes/, because Calc.java uses them.
set -euo pipefail

cd "$(dirname "$0")/.."

OUT_DIR=target/test-fixtures
mkdir -p "$OUT_DIR"
javac -d "$OUT_DIR" test/fixtures/Greeter.java
jar cf "$OUT_DIR/greeter.jar" -C "$OUT_DIR" Greeter.class
javac -cp target/emacs-jni-shim-classes -d "$OUT_DIR" test/fixtures/Calc.java
jar cf "$OUT_DIR/calc.jar" -C "$OUT_DIR" Calc.class
