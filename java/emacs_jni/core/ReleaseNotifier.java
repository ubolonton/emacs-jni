package emacs_jni.core;

// The `Cleaner` action of a `LispFunction`. After the function is collected, `run` tells Lisp to
// drop the handle from its function table.
final class ReleaseNotifier implements Runnable {
    private final long handle;

    ReleaseNotifier(long handle) {
        this.handle = handle;
    }

    @Override
    public native void run();
}
