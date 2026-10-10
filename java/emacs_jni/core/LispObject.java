package emacs_jni.core;

import java.lang.ref.Cleaner;

/**
 * An opaque Emacs Lisp value. Constructed only from Rust.
 *
 * <p>{@code ptr} is a boxed Rust {@code emacs::GlobalRef}, owned by this object. Exactly one
 * {@code LispObject} owns a given {@code ptr}, and its {@link Cleaner} frees it once. Do not add a
 * copy path (clone, serialization, a public constructor): two owners would free it twice.
 */
public final class LispObject {
    private static final Cleaner CLEANER = Cleaner.create();

    private final long ptr;

    /**
     * Takes ownership of {@code ptr}. Called only from Rust.
     *
     * @param ptr a boxed {@code emacs::GlobalRef}, from {@code Box::into_raw}
     */
    LispObject(long ptr) {
        this.ptr = ptr;
        CLEANER.register(this, new LispRefReleaseNotifier(ptr));
    }

    /**
     * Returns the Lisp printed form of this value. Blocks and throws as the {@link Lisp} methods
     * do.
     *
     * @return the result of {@code prin1-to-string}
     */
    public native String prin1ToString();

    /**
     * Returns the Lisp printed form of this value.
     *
     * @return the result of {@link #prin1ToString()}
     */
    @Override
    public String toString() {
        return prin1ToString();
    }
}
