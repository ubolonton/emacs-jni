package emacs_jni.core;

public final class LispError extends RuntimeException {
    public LispError(String message) {
        super(message);
    }
}
