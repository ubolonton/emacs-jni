package emacs_jni.core;

import java.util.concurrent.CompletableFuture;

final class LispFunction implements LispCallable {
    private final long handle;

    LispFunction(long handle) {
        this.handle = handle;
    }

    @Override
    public native CompletableFuture<Object> call(Object... args);
}
