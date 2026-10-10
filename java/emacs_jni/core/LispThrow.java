package emacs_jni.core;

// A Lisp `throw` that crosses to the JVM. See `LispSignal`.
public final class LispThrow extends LispNonLocalExit {
    public final Object tag;
    public final Object value;

    public LispThrow(Object tag, Object value) {
        super(THROW, tag, value);
        this.tag = tag;
        this.value = value;
    }
}
