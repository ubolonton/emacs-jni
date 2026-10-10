package emacs_jni.core;

/**
 * The {@link java.lang.ref.Cleaner} action of a {@link LispObject}. After the object is collected,
 * {@link #run} tells Emacs to free the {@code GlobalRef} behind {@code ptr}.
 */
final class LispRefReleaseNotifier implements Runnable {
    private final long ptr;

    /**
     * Watches one {@link LispObject}.
     *
     * @param ptr the {@code ptr} of the watched object
     */
    LispRefReleaseNotifier(long ptr) {
        this.ptr = ptr;
    }

    @Override
    public native void run();
}
