package emacs_jni.core;

import java.lang.ref.Cleaner;

// An opaque Emacs Lisp value. `ptr` is a boxed Rust `emacs::GlobalRef`
// (Box::into_raw), owned by this object -- there is no Rust-side handle
// table (see design spec, "Object model"). Constructed only from Rust
// (jvm_thread::build_lisp_reply_object), never by Java/Clojure code
// directly.
//
// INVARIANT (load-bearing, do not break): exactly one LispObject owns a
// given `ptr`. The one Cleaner registered below frees that GlobalRef
// once (via Inbound::FreeRef). `final` class + `final ptr` + a
// package-private constructor + no clone()/Cloneable/Serializable are
// what enforce it -- each Rust crossing mints a fresh GlobalRef, so two
// instances never share a pointer. Adding a copy path, or reusing a
// pointer across instances, would double-free.
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
