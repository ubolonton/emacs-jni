package emacs_jni.core;

import java.util.concurrent.CompletableFuture;

// Static entry points for Clojure to access Emacs Lisp values. Each op runs on the Emacs main
// thread. All but `applyAsync` block until the reply arrives.
public final class Lisp {
    private Lisp() {}

    // Values are `Object`: a symbol or keyword is a `LispSymbol`, a scalar is its boxed Java
    // value, and anything else is an opaque `LispObject`.
    public static native Object intern(String name);
    public static native Object typeOf(Object v);
    public static native boolean isNotNil(Object v);
    public static native boolean eq(Object a, Object b);
    public static native CompletableFuture<Object> applyAsync(Object f, Object[] args);
}
