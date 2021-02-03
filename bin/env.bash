set -euo pipefail

system=$(uname)
if [[ $system == "Linux" ]]; then
    ext="so"
elif [[ $system == "Darwin" ]]; then
    ext="dylib"
else
    echo "Unsupported system: $system"
    exit 1
fi

here=$(cd "$(dirname "$BASH_SOURCE")"; pwd)

PROJECT_ROOT=$(cd "$here/.."; pwd)
export PROJECT_ROOT

export TARGET=${TARGET:-debug}
export MODULE_DIR="$PROJECT_ROOT/target/$TARGET"
export MODULE_ORIGINAL=${MODULE_ORIGINAL:-libjni_dyn.$ext}
export MODULE_NAME=${MODULE_NAME:-jni-dyn}
export MODULE_RENAMED=${MODULE_NAME}.so
export TEST_FILE=jni-tests.el

export EMACS=${EMACS:-emacs}
