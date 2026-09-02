;;; jvm-callback-wrap-test.el --- Tests for wrapping a Lisp fn as a JVM object -*- lexical-binding: t; -*-

(require 'ert)
(require 'emacs-jni)

(defvar jvm-callback-wrap-test--calc-jar
  (expand-file-name "../target/test-fixtures/calc.jar"
                     (file-name-directory (or load-file-name buffer-file-name))))

(ert-deftest emacs-jni-jvm-callback-wrap-test/lisp-function-becomes-lispcallable ()
  (should (eq (jvm-start-sync) 'ready))
  (should (equal (jvm-load-jar-sync jvm-callback-wrap-test--calc-jar) '(t)))
  (should (equal (jvm-call-sync "Calc" "isLispCallable" (list (lambda (x) x)))
                  '(t t nil))))

(provide 'jvm-callback-wrap-test)
;;; jvm-callback-wrap-test.el ends here
