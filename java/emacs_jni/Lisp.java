package emacs_jni;

import java.util.concurrent.CompletableFuture;

// Static entry points for Clojure to hold and manipulate Emacs Lisp values
// (see design spec: "Built-in Lisp access from Clojure"). Every op is an
// emacs::Env method run on the Emacs main thread -- see Rust's
// lisp_requests::service. The sync natives (all but applyAsync) block
// internally on their own reply future (shim.rs's service_lisp_request_sync).
public final class Lisp {
    private Lisp() {}

    // Returns/accepts Object, not LispObject: a symbol or keyword comes
    // through as a LispSymbol marker (the dev.ublt.emacs.clj namespace maps
    // it to/from a clojure.lang.Symbol/Keyword), a scalar as its boxed Java
    // value, and anything else as an opaque LispObject.
    public static native Object intern(String name);
    public static native Object typeOf(Object v);
    public static native boolean isNotNil(Object v);
    public static native boolean eq(Object a, Object b);
    public static native CompletableFuture<Object> applyAsync(Object f, Object[] args);
}
