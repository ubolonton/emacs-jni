;;; jvm-invoke-test.el --- Tests for the JVM-calls-Lisp round trip -*- lexical-binding: t; -*-

(require 'ert)
(require 'jvm)

(defvar jvm-invoke-test--calc-jar
  (expand-file-name "../../target/test-fixtures/calc.jar"
                     (file-name-directory (or load-file-name buffer-file-name))))

(defun jvm-invoke-test--ensure-ready ()
  "Ensure the JVM is running and the Calc fixture jar is loaded.
Every test calls this: ERT does not run tests in definition order."
  (unless (eq (jvm-status) 'ready)
    (should (eq (jvm-start-await) 'ready)))
  (jvm-load-jar-await jvm-invoke-test--calc-jar))

(ert-deftest emacs-jni-jvm-invoke-test/java-calls-lisp-and-gets-the-result-back ()
  (jvm-invoke-test--ensure-ready)
  (should (equal (jvm-call-await "Calc" "callWithArg" (list (lambda (n) (* n 2)) 21))
                 42)))

(ert-deftest emacs-jni-jvm-invoke-test/lisp-error-propagates-as-lisp-signal ()
  "A failed Lisp callback reaches Java as a `LispSignal' with the original symbol."
  (jvm-invoke-test--ensure-ready)
  (should (equal (jvm-call-await "Calc" "describeCallbackError"
                                 (list (lambda (_n) (signal 'wrong-type-argument '(numberp "x"))) 1))
                 "wrong-type-argument")))

(ert-deftest emacs-jni-jvm-invoke-test/lisp-callback-error-message-is-the-condition ()
  "The message of the `LispSignal' from a failed callback is the condition."
  (jvm-invoke-test--ensure-ready)
  (should (equal (jvm-call-await "Calc" "callbackErrorMessage"
                                 (list (lambda (_n) (signal 'wrong-type-argument '(numberp "x"))) 1))
                 "(wrong-type-argument numberp \"x\")")))

(ert-deftest emacs-jni-jvm-invoke-test/java-calls-lisp-with-a-real-object-argument ()
  "A Java object passed to a Lisp callback arrives as a `user-ptr'."
  (jvm-invoke-test--ensure-ready)
  (let ((obj (jvm-new-await "Calc" '(7))))
    (should (user-ptrp obj))
    (should (equal (jvm-call-await obj "callWithSelf"
                                   (list (lambda (received)
                                           (if (user-ptrp received) "yes" "no"))))
                   "yes"))))

(ert-deftest emacs-jni-jvm-invoke-test/callback-returning-keyword-symbol-function ()
  "A JVM->Lisp callback may return a keyword, a symbol, or a function."
  (jvm-invoke-test--ensure-ready)
  ;; Keyword and symbol round-trip as themselves.
  (should (eq (jvm-call-await "Calc" "callWithArg" (list (lambda (_n) :a) 1)) :a))
  (should (eq (jvm-call-await "Calc" "callWithArg" (list (lambda (_n) 'foo) 1)) 'foo))
  ;; A function returns to Java as a `LispFunction', and back as a `user-ptr'.
  (should (user-ptrp (jvm-call-await "Calc" "callWithArg"
                                     (list (lambda (_n) (lambda () 1)) 1)))))

(provide 'jvm-invoke-test)
;;; jvm-invoke-test.el ends here
