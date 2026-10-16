;;; jvm-symbol-marshal-test.el --- Tests for symbol/keyword marshalling -*- lexical-binding: t; -*-

;; The generic bidirectional round trip through `clojure.core/identity'
;; (Lisp keyword/symbol -> LispSymbol marker -> Java -> back), which needs a
;; running JVM + the Clojure jars.

(require 'ert)
(require 'jvm)
(require 'jvm-clj)

(defvar jvm-symbol-marshal-test--ready nil)

(defun jvm-symbol-marshal-test--ensure-ready ()
  (unless jvm-symbol-marshal-test--ready
    (should (eq (jvm-start-await) 'ready))
    (dolist (jar (jvm-clj-ensure-jars))
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
