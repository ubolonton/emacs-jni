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
}
