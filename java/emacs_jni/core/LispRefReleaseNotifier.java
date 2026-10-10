package emacs_jni.core;

// The `Cleaner` action of a `LispObject`. After the object is collected, `run` tells Emacs to free
// the `GlobalRef` behind `ptr`.
final class LispRefReleaseNotifier implements Runnable {
    private final long ptr;

    LispRefReleaseNotifier(long ptr) {
        this.ptr = ptr;
    }

    @Override
    public native void run();
}
