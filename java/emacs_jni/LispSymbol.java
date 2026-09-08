package emacs_jni;

// Wire form for a Lisp symbol or keyword crossing to/from Clojure. The real
// clojure.lang.Symbol/Keyword live in the child URLClassLoader, unreachable
// from a native method's or the worker thread's find_class, so this
// platform-loader shim carries just the name plus a keyword flag; the
// dev.ublt.emacs.clj namespace does the final (symbol name)/(keyword name)
// construction and destructuring. `name` is the bare name -- no leading
// ":" for a keyword. Constructed from Rust (jvm_thread::build_lisp_reply_object)
// and from the namespace's arg marshalling; a pure data carrier, no natives.
public final class LispSymbol {
    public final String name;
    public final boolean keyword;

    public LispSymbol(String name, boolean keyword) {
        this.name = name;
        this.keyword = keyword;
    }
}
