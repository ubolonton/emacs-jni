package emacs_jni.core;

/**
 * A Lisp {@code signal} that crosses to the JVM. {@link #symbol} and {@link #data} are a
 * {@link LispSymbol}, a {@link LispObject}, or a boxed scalar. The message is the condition, as in
 * {@code (error "boom")}.
 */
// Not meant to be serialized. See LispNonLocalExit.
@SuppressWarnings("serial")
public final class LispSignal extends LispNonLocalExit {
    /** The error symbol, for example {@code wrong-type-argument}. */
    public final Object symbol;

    /** The error data. */
    public final Object data;

    /**
     * Creates a signal. Rust calls this when a Lisp {@code signal} reaches Java.
     *
     * @param symbol the error symbol
     * @param data the error data
     * @param message the condition, printed by Lisp, or {@code null}
     */
    public LispSignal(Object symbol, Object data, String message) {
        super(SIGNAL, symbol, data, message);
        this.symbol = symbol;
        this.data = data;
    }
}
