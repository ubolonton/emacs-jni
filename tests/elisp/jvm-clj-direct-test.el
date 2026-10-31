;;; jvm-clj-direct-test.el --- Reentrant Clojure->Lisp over jvm-call-direct -*- lexical-binding: t; -*-

;; Clojure calls back into Lisp during a `jvm-call-direct'. The main thread
;; is blocked in the JVM, so it cannot drain the async queue; the `Lisp.*'
;; natives run the request directly (`lisp::runtime'). If that breaks,
;; these tests deadlock instead of failing.
;;
;; These tests check how a Lisp non-local exit leaves the direct call.  The
;; Clojure tests in tests/clojure/ cover the callbacks themselves, on the
;; `direct' path of jvm-clj-suite-test.el.

(require 'ert)
(require 'jvm)
(require 'jvm-clj)

(defun jvm-clj-direct-test--ensure-ready ()
  (unless (eq (jvm-status) 'ready)
    (jvm-clj-start-await)))

(defun jvm-clj-direct-test--eval (code)
  "Evaluate CODE (Clojure source) with `jvm-call-direct' at every step."
  (let ((evalfn (jvm-call-direct "clojure.java.api.Clojure" "var"
                                 (list "clojure.core" "eval")))
        (form (jvm-call-direct "clojure.java.api.Clojure" "read" (list code))))
    (jvm-call-direct evalfn "invoke" (list form))))

(ert-deftest emacs-jni-jvm-clj-direct-test/reentrant-lisp-signal-propagates-out ()
  "A Lisp signal in a reentrant callback is re-raised as itself,
not as `jvm-error'."
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
