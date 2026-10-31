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

    // Same arity as `Calc(long)`, for the constructor overload fall-through test.
    public Calc(String label) {
        this.base = label.length();
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
        // Blocking is acceptable in a test fixture.
        return fn.call(n).get(10, java.util.concurrent.TimeUnit.SECONDS);
    }

    /** Calls {@code fn} with a Java object ({@code this}), not a primitive. */
    public Object callWithSelf(emacs_jni.core.LispCallable fn) throws Exception {
        return fn.call(this).get(10, java.util.concurrent.TimeUnit.SECONDS);
    }

    // Same-arity overloads, for the method overload fall-through test.
    public static String pick(long n) {
        return "long";
    }

    public static String pick(String s) {
        return "string";
    }

    /** Returns the symbol name of the {@code LispSignal} from a failure of {@code fn}. */
    public static String describeCallbackError(emacs_jni.core.LispCallable fn, long n) throws Exception {
        try {
            fn.call(n).get(10, java.util.concurrent.TimeUnit.SECONDS);
            return "no-exception";
        } catch (java.util.concurrent.ExecutionException e) {
            Throwable cause = e.getCause();
            if (cause instanceof emacs_jni.core.LispSignal) {
                Object symbol = ((emacs_jni.core.LispSignal) cause).symbol;
                if (symbol instanceof emacs_jni.core.LispSymbol) {
                    return ((emacs_jni.core.LispSymbol) symbol).name;
                }
                return "non-symbol:" + symbol;
            }
            return "not-a-lisp-signal:" + cause;
        }
    }

    /** Returns the message of the exception from a failure of {@code fn}. */
    public static String callbackErrorMessage(emacs_jni.core.LispCallable fn, long n) throws Exception {
        try {
            fn.call(n).get(10, java.util.concurrent.TimeUnit.SECONDS);
            return "no-exception";
        } catch (java.util.concurrent.ExecutionException e) {
            return e.getCause().getMessage();
        }
    }
}
