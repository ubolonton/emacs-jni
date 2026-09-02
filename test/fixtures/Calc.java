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
        return obj instanceof emacs_jni.LispCallable;
    }

    public static Object callWithArg(emacs_jni.LispCallable fn, long n) throws Exception {
        // A synchronous-looking wait is fine INSIDE A TEST FIXTURE (not
        // inside the shim or any real interop path) -- this just proves
        // the round trip completes with the right value; production Java
        // code calling a LispCallable would normally chain
        // .thenAccept(...) instead of blocking.
        return fn.call(n).get(10, java.util.concurrent.TimeUnit.SECONDS);
    }
}
