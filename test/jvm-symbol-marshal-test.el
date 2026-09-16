;;; jvm-symbol-marshal-test.el --- Tests for symbol/keyword marshalling -*- lexical-binding: t; -*-

;; Two layers: the pure wire-reify arm (`jvm--resolve-sexp-result', no JVM),
;; and the generic bidirectional round trip through `clojure.core/identity'
;; (Lisp keyword/symbol -> LispSymbol marker -> Java -> back), which needs a
;; running JVM + the Clojure jars.

(require 'ert)
(require 'jvm)
(require 'jvm-clj)

(ert-deftest emacs-jni-jvm-symbol-marshal/reify-wire-tag ()
  "The `jvm-symbol' wire tag reifies to an interned keyword/symbol."
  (should (eq (jvm--resolve-sexp-result '(jvm-symbol "foo" t)) :foo))
  (should (eq (jvm--resolve-sexp-result '(jvm-symbol "foo" nil)) 'foo)))

(defvar jvm-symbol-marshal-test--jars-dir (getenv "EMACS_JNI_CLOJURE_JARS_DIR"))
(defvar jvm-symbol-marshal-test--ready nil)

(defun jvm-symbol-marshal-test--ensure-ready ()
  (unless (and jvm-symbol-marshal-test--jars-dir
               (file-directory-p jvm-symbol-marshal-test--jars-dir))
    (ert-skip "Set EMACS_JNI_CLOJURE_JARS_DIR (see test/fetch-clojure-jars.sh) to run this test"))
  (unless jvm-symbol-marshal-test--ready
    (should (eq (jvm-start-await) 'ready))
    (dolist (jar (directory-files jvm-symbol-marshal-test--jars-dir t "\\.jar\\'"))
      (jvm-load-jar-await jar))
    (setq jvm-symbol-marshal-test--ready t)))

(ert-deftest emacs-jni-jvm-symbol-marshal/identity-round-trips-symbols ()
  "A Lisp keyword/symbol passed to `clojure.core/identity' via the generic
bridge crosses as a marker and comes back as the same interned value. A
function symbol (`car') is data here, not a callback."
  (jvm-symbol-marshal-test--ensure-ready)
  (let ((identity-fn (jvm-call-direct "clojure.java.api.Clojure" "var"
                                      (list "clojure.core" "identity"))))
    (should (eq (jvm-call-direct identity-fn "invoke" (list :a)) :a))
    (should (eq (jvm-call-direct identity-fn "invoke" (list 'foo)) 'foo))
    (should (eq (jvm-call-direct identity-fn "invoke" (list 'car)) 'car))))

(provide 'jvm-symbol-marshal-test)
;;; jvm-symbol-marshal-test.el ends here
