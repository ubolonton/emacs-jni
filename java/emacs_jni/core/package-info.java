/**
 * Java types for Emacs Lisp values and calls into Lisp.
 *
 * <p>Rust defines these classes in the platform classloader when the JVM starts, so jar classes
 * and Clojure code can see them. Public API: {@link emacs_jni.core.LispCallable},
 * {@link emacs_jni.core.Lisp}, {@link emacs_jni.core.LispObject},
 * {@link emacs_jni.core.LispSymbol}, and the {@link emacs_jni.core.LispNonLocalExit} exceptions.
 */
package emacs_jni.core;
