package emacs_jni.core;

/**
 * Base class of a Lisp non-local exit ({@link LispSignal}, {@link LispThrow}) that crosses to the
 * JVM. Catch this class to handle both kinds.
 *
 * <p>The class is in the platform classloader, so Rust can construct it without the Clojure
 * classloader.
 */
public class LispNonLocalExit extends RuntimeException {
    /** The {@link #kind} of a {@link LispSignal}. */
    public static final int SIGNAL = 0;

    /** The {@link #kind} of a {@link LispThrow}. */
    public static final int THROW = 1;

    /** {@link #SIGNAL} or {@link #THROW}. Rust reads it to raise the same exit again in Lisp. */
    public final int kind;

    /**
     * The error symbol of a signal, or the tag of a throw: a {@link LispSymbol}, a
     * {@link LispObject}, or a boxed scalar.
     */
    public final Object symbolOrTag;

    /** The data of a signal, or the value of a throw. Same types as {@link #symbolOrTag}. */
    public final Object dataOrValue;

    /**
     * Creates an exit of the given kind. Use the {@link LispSignal} or {@link LispThrow}
     * constructor instead.
     *
     * @param kind {@link #SIGNAL} or {@link #THROW}
     * @param symbolOrTag the error symbol or the catch tag
     * @param dataOrValue the error data or the thrown value
     */
    public LispNonLocalExit(int kind, Object symbolOrTag, Object dataOrValue) {
        super();
        this.kind = kind;
        this.symbolOrTag = symbolOrTag;
        this.dataOrValue = dataOrValue;
    }
}
