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

(provide 'jvm-symbol-marshal-test)
;;; jvm-symbol-marshal-test.el ends here
