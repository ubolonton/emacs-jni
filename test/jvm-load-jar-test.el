;;; jvm-load-jar-test.el --- Tests for jvm-load-jar -*- lexical-binding: t; -*-

(require 'ert)
(require 'jvm)

(ert-deftest emacs-jni-jvm-load-jar-test/loads-fixture-jar ()
  (should (eq (jvm-start-await) 'ready))
  (should (null (jvm-load-jar-await (getenv "EMACS_JNI_GREETER_JAR")))))

(ert-deftest emacs-jni-jvm-load-jar-test/reports-failure-for-missing-jar ()
  (should (eq (jvm-status) 'ready))
  (let ((err (should-error (jvm-load-jar-await "/nonexistent/path/does-not-exist.jar")
                           :type 'jvm-error)))
    (should (stringp (cadr err)))))

(provide 'jvm-load-jar-test)
;;; jvm-load-jar-test.el ends here
