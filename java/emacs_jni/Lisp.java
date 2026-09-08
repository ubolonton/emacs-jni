package emacs_jni;

import java.util.concurrent.CompletableFuture;

// Static entry points for Clojure to hold and manipulate Emacs Lisp values
// (see design spec: "Built-in Lisp access from Clojure"). Every op is an
// emacs::Env method run on the Emacs main thread -- see Rust's
// lisp_requests::service. The sync natives (all but applyAsync) block
// internally on their own reply future (shim.rs's service_lisp_request_sync).
public final class Lisp {
    private Lisp() {}

    public static native LispObject intern(String name);
    public static native LispObject typeOf(LispObject v);
    public static native boolean isNotNil(LispObject v);
    public static native boolean eq(LispObject a, LispObject b);
    public static native CompletableFuture<LispObject> applyAsync(LispObject f, Object[] args);
}
