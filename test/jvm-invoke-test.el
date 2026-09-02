;;; jvm-invoke-test.el --- Tests for the JVM-calls-Lisp round trip -*- lexical-binding: t; -*-

(require 'ert)
(require 'emacs-jni)

(defvar jvm-invoke-test--calc-jar
  (expand-file-name "../target/test-fixtures/calc.jar"
                     (file-name-directory (or load-file-name buffer-file-name))))

(ert-deftest emacs-jni-jvm-invoke-test/java-calls-lisp-and-gets-the-result-back ()
  (should (eq (jvm-start-sync) 'ready))
  (should (equal (jvm-load-jar-sync jvm-invoke-test--calc-jar) '(t)))
  (should (equal (jvm-call-sync "Calc" "callWithArg" (list (lambda (n) (* n 2)) 21))
                  '(t 42 nil))))

(ert-deftest emacs-jni-jvm-invoke-test/lisp-error-propagates-as-java-exception ()
  (should (eq (jvm-status) 'ready))
  (let ((result (jvm-call-sync "Calc" "callWithArg"
                                 (list (lambda (_n) (error "boom")) 1))))
    (should (eq (nth 0 result) nil))
    (should (string-match-p "boom" (nth 2 result)))))

(provide 'jvm-invoke-test)
;;; jvm-invoke-test.el ends here
