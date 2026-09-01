package emacs_jni;

import java.util.concurrent.CompletableFuture;

public interface LispCallable {
    CompletableFuture<Object> call(Object... args);
}
