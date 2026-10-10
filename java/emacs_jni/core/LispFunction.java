package emacs_jni.core;

import java.util.concurrent.CompletableFuture;

/**
 * The {@link LispCallable} for a Lisp function. Constructed only from Rust. {@code handle} is the
 * key of the function in the Lisp table {@code jvm--lisp-callbacks}.
 */
final class LispFunction implements LispCallable {
    private final long handle;

    /**
     * Wraps a callback handle. Called only from Rust.
     *
     * @param handle the key in {@code jvm--lisp-callbacks}
     */
    LispFunction(long handle) {
        this.handle = handle;
    }

    @Override
    public native CompletableFuture<Object> call(Object... args);
}
