package emacs_jni.core;

/**
 * A Lisp {@code throw} that crosses to the JVM. {@link #tag} and {@link #value} have the same
 * types as in {@link LispSignal}.
 */
public final class LispThrow extends LispNonLocalExit {
    /** The catch tag. */
    public final Object tag;

    /** The thrown value. */
    public final Object value;

    /**
     * Creates a throw. Rust calls this when a Lisp {@code throw} reaches Java.
     *
     * @param tag the catch tag
     * @param value the thrown value
     */
    public LispThrow(Object tag, Object value) {
        super(THROW, tag, value);
        this.tag = tag;
        this.value = value;
    }
}
