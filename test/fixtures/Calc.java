public class Calc {
    public static long addLong(long a, long b) {
        return a + b;
    }

    public static java.math.BigInteger addBig(java.math.BigInteger a, java.math.BigInteger b) {
        return a.add(b);
    }

    private final long base;

    public Calc(long base) {
        this.base = base;
    }

    public long addToBase(long n) {
        return base + n;
    }

    public static String describe(boolean flag) {
        return flag ? "yes" : "no";
    }

    public static boolean isLispCallable(Object obj) {
        return obj instanceof emacs_jni.core.LispCallable;
    }

    public static Object callWithArg(emacs_jni.core.LispCallable fn, long n) throws Exception {
        // A synchronous-looking wait is fine INSIDE A TEST FIXTURE (not
        // inside the shim or any real interop path) -- this just proves
        // the round trip completes with the right value; production Java
        // code calling a LispCallable would normally chain
        // .thenAccept(...) instead of blocking.
        return fn.call(n).get(10, java.util.concurrent.TimeUnit.SECONDS);
    }

    // Regression fixture for Finding 1 (whole-branch review): calls FN with
    // a real Java object (`this`, a Calc instance) rather than a primitive,
    // so the Lisp callback receives a `(jvm-handle N)` wire tag as one of
    // its ARGS -- see `jvm--dispatch-message`'s `invoke` case in
    // elisp/emacs-jni.el.
    public Object callWithSelf(emacs_jni.core.LispCallable fn) throws Exception {
        return fn.call(this).get(10, java.util.concurrent.TimeUnit.SECONDS);
    }

    // Regression fixture for Finding 2 (whole-branch review): two same-
    // name, same-arity overloads with different parameter types, so
    // `invoke_method` must fall through to the next candidate if the first
    // one `getMethods()` returns doesn't fit the actual argument type.
    public static String pick(long n) {
        return "long";
    }

    public static String pick(String s) {
        return "string";
    }
}
