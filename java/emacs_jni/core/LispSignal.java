package emacs_jni.core;

// A Lisp `signal' crossing to the JVM. Extends the neutral base so a single
// `catch (LispNonLocalExit)' still works; adds typed `symbol'/`data' fields
// (already marshalled: a LispSymbol marker, a LispObject, or a boxed scalar).
public final class LispSignal extends LispNonLocalExit {
    public final Object symbol;
    public final Object data;

    public LispSignal(Object symbol, Object data) {
        super(SIGNAL, symbol, data);
        this.symbol = symbol;
        this.data = data;
    }
}
