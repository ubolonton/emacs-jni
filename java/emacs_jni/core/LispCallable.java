package emacs_jni.core;

import java.util.concurrent.CompletableFuture;

/**
 * A Lisp function that Java code can call. A Lisp function passed as a {@code jvm-call} argument
 * arrives as a {@code LispCallable}.
 */
public interface LispCallable {
    /**
     * Calls the Lisp function with {@code args}. The call runs on the Emacs main thread.
     *
     * <p>Does not block. Inside {@code jvm-call-direct}, the function runs before this method
     * returns, and the future is already complete.
     *
     * @param args the arguments, converted to Lisp values as for {@code jvm-call} results
     * @return a future of the return value. It completes exceptionally with a {@link LispSignal}
     *     or {@link LispThrow} if the function exits non-locally.
     */
    CompletableFuture<Object> call(Object... args);
}
