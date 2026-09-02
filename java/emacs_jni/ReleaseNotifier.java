package emacs_jni;

// Registered as a java.lang.ref.Cleaner action on a LispFunction proxy (see
// marshal.rs's register_release_on_gc). When the proxy becomes
// phantom-reachable, the Cleaner runs this on a JVM-managed background
// thread; the native run() tells Lisp (over the worker-to-Lisp channel) to
// drop its own function-table entry for the same handle -- the JVM-side half
// of the GC-notification loop, mirroring JvmHandle's Drop impl on the Rust
// side.
final class ReleaseNotifier implements Runnable {
    private final long handle;

    ReleaseNotifier(long handle) {
        this.handle = handle;
    }

    @Override
    public native void run();
}
