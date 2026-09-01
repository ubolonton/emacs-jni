package emacs_jni;

public final class LispError extends RuntimeException {
    public LispError(String message) {
        super(message);
    }
}
