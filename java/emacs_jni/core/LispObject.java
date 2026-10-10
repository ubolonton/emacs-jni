package emacs_jni.core;

import java.lang.ref.Cleaner;

// An opaque Emacs Lisp value. `ptr` is a boxed Rust `emacs::GlobalRef`, owned by this object.
// Constructed only from Rust.
//
// Exactly one `LispObject` owns a given `ptr`, and its `Cleaner` frees it once. Do not add a
// copy path (clone, serialization, a public constructor): two owners would double-free.
public final class LispObject {
    private static final Cleaner CLEANER = Cleaner.create();

    private final long ptr;

    LispObject(long ptr) {
        this.ptr = ptr;
        CLEANER.register(this, new LispRefReleaseNotifier(ptr));
    }

    public native String prin1ToString();

    @Override
    public String toString() {
        return prin1ToString();
    }
}
