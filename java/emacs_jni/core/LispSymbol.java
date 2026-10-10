package emacs_jni.core;

// A Lisp symbol or keyword crossing to or from Clojure. `clojure.lang.Symbol` and `Keyword` are
// in the jar classloader, which Rust cannot reach, so this platform-loader class carries the name
// and a keyword flag. The `emacs-jni.clj` namespace converts it. `name` has no leading `:`.
public final class LispSymbol {
    public final String name;
    public final boolean keyword;

    public LispSymbol(String name, boolean keyword) {
        this.name = name;
        this.keyword = keyword;
    }
}
