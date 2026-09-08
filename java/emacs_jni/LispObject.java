package emacs_jni;

import java.lang.ref.Cleaner;

// An opaque Emacs Lisp value. `ptr` is a boxed Rust `emacs::GlobalRef`
// (Box::into_raw), owned by this object -- there is no Rust-side handle
// table (see design spec, "Object model"). Constructed only from Rust
// (jvm_thread::build_lisp_reply_object), never by Java/Clojure code
// directly.
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
