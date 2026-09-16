#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${1:-$(dirname "$0")/../target/clojure-jars}"
mkdir -p "$OUT_DIR"

MAVEN=https://repo1.maven.org/maven2
CLOJARS=https://repo.clojars.org

fetch() {
    local url="$1" out="$OUT_DIR/$2"
    if [ -f "$out" ]; then
        echo "already have $2"
        return
    fi
    echo "fetching $2"
    curl -sf -o "$out" "$url"
}

# Verified-working versions — see docs/superpowers/specs/2026-09-02-clojure-nrepl-design.md
fetch "$MAVEN/org/clojure/clojure/1.12.0/clojure-1.12.0.jar" clojure-1.12.0.jar
fetch "$MAVEN/org/clojure/spec.alpha/0.5.238/spec.alpha-0.5.238.jar" spec.alpha-0.5.238.jar
fetch "$MAVEN/org/clojure/core.specs.alpha/0.4.74/core.specs.alpha-0.4.74.jar" core.specs.alpha-0.4.74.jar
# nREPL is published to Clojars, NOT Maven Central.
fetch "$CLOJARS/nrepl/nrepl/1.7.0/nrepl-1.7.0.jar" nrepl-1.7.0.jar

echo "Jars ready in $OUT_DIR"
echo "Run Clojure/nREPL tests with:"
echo "  EMACS_JNI_CLOJURE_JARS_DIR=$OUT_DIR ./test/run-ert-tests.sh test/jvm-clj-eval-test.el"
