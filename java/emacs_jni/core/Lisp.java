package emacs_jni.core;

import java.util.concurrent.CompletableFuture;

/**
 * Static entry points for Clojure to access Emacs Lisp values. Each operation runs on the Emacs
 * main thread.
 *
 * <p>A value is an {@code Object}: a symbol or keyword is a {@link LispSymbol}, a scalar is its
 * boxed Java value, and any other value is an opaque {@link LispObject}. Lisp {@code nil} is
 * {@code null}, and {@code t} is {@link Boolean#TRUE}.
 *
 * <p>All methods except {@link #applyAsync} block until the reply arrives, for a maximum of 30
 * seconds. These methods do not declare checked exceptions, but they can throw:
 *
 * <ul>
 *   <li>{@link LispSignal} or {@link LispThrow}, if the Lisp operation exits non-locally;
 *   <li>{@link java.util.concurrent.TimeoutException}, if no reply arrives in time;
 *   <li>{@link InterruptedException}, if the thread is interrupted while it waits.
 * </ul>
 */
public final class Lisp {
    private Lisp() {}

    /**
     * Interns {@code name} in the global obarray.
     *
     * @param name the symbol name. A leading {@code :} makes a keyword.
     * @return the symbol, as a {@link LispSymbol}, {@code null} for {@code nil}, or
     *     {@link Boolean#TRUE} for {@code t}
     */
    public static native Object intern(String name);

    /**
     * Returns the Lisp {@code type-of} of {@code v}.
     *
     * @param v a Lisp value
     * @return the type symbol, as a {@link LispSymbol}
     */
    public static native Object typeOf(Object v);

    /**
     * Tests whether {@code v} is not Lisp {@code nil}.
     *
     * @param v a Lisp value
     * @return {@code true} unless {@code v} is {@code nil}
     */
    public static native boolean isNotNil(Object v);

    /**
     * Tests whether {@code a} and {@code b} are the same Lisp object (Lisp {@code eq}).
     *
     * @param a a Lisp value
     * @param b a Lisp value
     * @return {@code true} if they are {@code eq}
     */
    public static native boolean eq(Object a, Object b);

    /**
     * Applies the Lisp function {@code f} to {@code args}. Does not block.
     *
     * @param f a function {@link LispObject}, or a {@link LispSymbol} that names a function
     * @param args the arguments
     * @return a future of the result. It completes exceptionally with a {@link LispSignal} or
     *     {@link LispThrow} if {@code f} exits non-locally.
     */
    public static native CompletableFuture<Object> applyAsync(Object f, Object[] args);
}
