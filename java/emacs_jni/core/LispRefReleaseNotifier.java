package emacs_jni.core;

// Registered as a java.lang.ref.Cleaner action on a LispObject (see
// LispObject's constructor). When the LispObject becomes phantom-reachable,
// the Cleaner runs this on a JVM-managed background thread; the native
// run() enqueues a FreeRef request (Rust's lisp_requests queue) and wakes
// the main thread up, so it frees the underlying GlobalRef exactly once --
// the Lisp-access counterpart of ReleaseNotifier's role for LispFunction.
final class LispRefReleaseNotifier implements Runnable {
    private final long ptr;

    LispRefReleaseNotifier(long ptr) {
        this.ptr = ptr;
    }

    @Override
    public native void run();
}
