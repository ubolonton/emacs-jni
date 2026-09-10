package emacs_jni.core;

// Neutral carrier for a Lisp non-local exit crossing to Clojure. Built by
// Rust (jvm_thread), completed onto the reply future via
// completeExceptionally. The emacs-jni.clj namespace rethrows it as an
// ex-info; this class stays a plain platform-loader RuntimeException so
// Rust never needs to reach the Clojure classloader. `symbolOrTag` and
// `dataOrValue` are already marshaled (a LispSymbol marker, a LispObject,
// or a boxed scalar).
public final class LispNonLocalExit extends RuntimeException {
    public static final int SIGNAL = 0;
    public static final int THROW = 1;

    public final int kind;
    public final Object symbolOrTag;
    public final Object dataOrValue;

    public LispNonLocalExit(int kind, Object symbolOrTag, Object dataOrValue) {
        super();
        this.kind = kind;
        this.symbolOrTag = symbolOrTag;
        this.dataOrValue = dataOrValue;
    }
}
