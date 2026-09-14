;;; jvm-clj-direct-test.el --- Reentrant Clojure->Lisp over jvm-call-direct -*- lexical-binding: t; -*-

;; The direct (main-thread) counterpart of the reentrancy test in
;; jvm-clj-test.el. Here the OUTER call is a synchronous `jvm-call-direct',
;; so the main thread is blocked inside the JVM for its whole duration. The
;; Clojure code calls back into Lisp *during* that call. The async queue
;; cannot be drained by the blocked main thread, so this only works because
;; the `Lisp.*' natives run the callback directly on the enclosing frame's
;; Env (see src/reentry.rs, src/shim.rs). Without that fast path these tests
;; would deadlock, not fail.

(require 'ert)
(require 'jvm)
(require 'jvm-clj)

(defvar jvm-clj-direct-test--jars-dir (getenv "EMACS_JNI_CLOJURE_JARS_DIR"))

(defun jvm-clj-direct-test--ensure-ready ()
  (unless (and jvm-clj-direct-test--jars-dir
               (file-directory-p jvm-clj-direct-test--jars-dir))
    (ert-skip "Set EMACS_JNI_CLOJURE_JARS_DIR (see test/fetch-clojure-jars.sh)"))
  (unless (eq (jvm-status) 'ready)
    (setq jvm-clj-classpath (directory-files jvm-clj-direct-test--jars-dir t "\\.jar\\'"))
    (jvm-clj-start)))

(defun jvm-clj-direct-test--eval (code)
  "Evaluate CODE (Clojure source) with `jvm-call-direct' at every step, so
the whole Lisp->Clojure call runs synchronously on this thread. Reads CODE
via `clojure.java.api.Clojure/read' (edn), resolves `clojure.core/eval',
and invokes it -- the direct-call equivalent of `jvm-clj-eval-string'."
  (let ((evalfn (jvm-call-direct "clojure.java.api.Clojure" "var"
                                 (list "clojure.core" "eval")))
        (form (jvm-call-direct "clojure.java.api.Clojure" "read" (list code))))
    (jvm-call-direct evalfn "invoke" (list form))))

(ert-deftest emacs-jni-jvm-clj-direct-test/reentrant-sync-natives-do-not-deadlock ()
  "intern + is-not-nil? called back from Clojure, inside a direct call."
  (jvm-clj-direct-test--ensure-ready)
  (should (eq (jvm-clj-direct-test--eval
               "(let [sym (emacs-jni.clj/intern \"car\")]
                  (emacs-jni.clj/is-not-nil? sym))")
              t)))

(ert-deftest emacs-jni-jvm-clj-direct-test/reentrant-apply-returns-a-value ()
  "apply (applyAsync + deref) back into Lisp resolves synchronously."
  (jvm-clj-direct-test--ensure-ready)
  (should (equal (jvm-clj-direct-test--eval
                  "(emacs-jni.clj/apply (symbol \"concat\") \"a\" \"b\")")
                 "ab")))

(ert-deftest emacs-jni-jvm-clj-direct-test/reentrant-require-symbol-is-callable ()
  (jvm-clj-direct-test--ensure-ready)
  (should (= (jvm-clj-direct-test--eval
              "((emacs-jni.clj/require-symbol (quote +)) 1 2 3)")
             6)))

(ert-deftest emacs-jni-jvm-clj-direct-test/reentrant-lisp-signal-propagates-out ()
  "A Lisp signal raised in a reentrant callback unwinds through Clojure as a
LispNonLocalExit and is reconstructed at the direct-call boundary as the
*original* signal -- symbol `error' with data (\"boom\") -- not a generic
`jvm-error'."
  (jvm-clj-direct-test--ensure-ready)
  (let ((err (should-error (jvm-clj-direct-test--eval
                            "(emacs-jni.clj/apply (symbol \"error\") \"boom\")")
                           :type 'error)))
    ;; `should-error' returns the caught condition as (SYMBOL . DATA).
    (should (equal err '(error "boom")))))

(ert-deftest emacs-jni-jvm-clj-direct-test/reentrant-lisp-throw-propagates-out ()
  "A Lisp `throw' from a reentrant callback is reconstructed at the direct-call
boundary as the original tag+value, so an enclosing `catch' receives it."
  (jvm-clj-direct-test--ensure-ready)
  (should (= 42 (catch 'my-tag
                  (jvm-clj-direct-test--eval
                   "(emacs-jni.clj/apply (symbol \"throw\") (quote my-tag) 42)")))))

(provide 'jvm-clj-direct-test)
;;; jvm-clj-direct-test.el ends here
