;;; jvm-load-jar-test.el --- Tests for jvm-load-jar -*- lexical-binding: t; -*-

(require 'ert)
(require 'jvm)

(ert-deftest emacs-jni-jvm-load-jar-test/loads-fixture-jar ()
  (should (eq (jvm-start-await) 'ready))
  (let ((result (jvm-load-jar-await (getenv "EMACS_JNI_GREETER_JAR"))))
    (should (eq (car result) t))
    (should (null (cdr result)))))

(ert-deftest emacs-jni-jvm-load-jar-test/reports-failure-for-missing-jar ()
  (should (eq (jvm-status) 'ready))
  (let ((result (jvm-load-jar-await "/nonexistent/path/does-not-exist.jar")))
    (should (eq (car result) nil))
    (should (stringp (cdr result)))))

(provide 'jvm-load-jar-test)
;;; jvm-load-jar-test.el ends here
