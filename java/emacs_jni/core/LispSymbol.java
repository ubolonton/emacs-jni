package emacs_jni.core;

/**
 * A Lisp symbol or keyword that crosses to or from Clojure.
 *
 * <p>{@code clojure.lang.Symbol} and {@code Keyword} are in the jar classloader, which Rust cannot
 * reach. So this platform-loader class carries the name and a keyword flag, and the
 * {@code emacs-jni.clj} namespace converts it.
 */
public final class LispSymbol {
    /** The symbol name. A keyword name has no leading {@code :}. */
    public final String name;

    /** {@code true} for a keyword. */
    public final boolean keyword;

    /**
     * Creates a symbol or keyword.
     *
     * @param name the name, with no leading {@code :}
     * @param keyword {@code true} for a keyword
     */
    public LispSymbol(String name, boolean keyword) {
        this.name = name;
        this.keyword = keyword;
    }
}
