package emacs_jni.core;

/**
 * A Lisp {@code throw} that crosses to the JVM. {@link #tag} and {@link #value} have the same
 * types as in {@link LispSignal}. The message is the throw, as in {@code (throw my-tag 42)}.
 */
// Not meant to be serialized. See LispNonLocalExit.
@SuppressWarnings("serial")
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
     * @param message the throw, printed by Lisp, or {@code null}
     */
    public LispThrow(Object tag, Object value, String message) {
        super(THROW, tag, value, message);
        this.tag = tag;
        this.value = value;
    }
}
