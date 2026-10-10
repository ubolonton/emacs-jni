package emacs_jni.core;

// A Lisp `signal` that crosses to the JVM. `symbol` and `data` are a `LispSymbol`, a
// `LispObject`, or a boxed scalar.
public final class LispSignal extends LispNonLocalExit {
    public final Object symbol;
    public final Object data;

    public LispSignal(Object symbol, Object data) {
        super(SIGNAL, symbol, data);
        this.symbol = symbol;
        this.data = data;
    }
}
