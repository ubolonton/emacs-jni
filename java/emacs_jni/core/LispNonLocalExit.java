package emacs_jni.core;

// Base class of a Lisp non-local exit (`LispSignal`, `LispThrow`) that crosses to the JVM. It is
// in the platform loader, so Rust can construct it without the Clojure classloader.
// `symbolOrTag` and `dataOrValue` are a `LispSymbol`, a `LispObject`, or a boxed scalar.
public class LispNonLocalExit extends RuntimeException {
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
