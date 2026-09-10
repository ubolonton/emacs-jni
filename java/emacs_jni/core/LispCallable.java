package emacs_jni.core;

import java.util.concurrent.CompletableFuture;

public interface LispCallable {
    CompletableFuture<Object> call(Object... args);
}
