package emacs_jni.core;

/**
 * The {@link java.lang.ref.Cleaner} action of a {@link LispFunction}. After the function is
 * collected, {@link #run} tells Lisp to remove {@code handle} from {@code jvm--lisp-callbacks}.
 */
final class ReleaseNotifier implements Runnable {
    private final long handle;

    /**
     * Watches one {@link LispFunction}.
     *
     * @param handle the handle of the watched function
     */
    ReleaseNotifier(long handle) {
        this.handle = handle;
    }

    @Override
    public native void run();
}
