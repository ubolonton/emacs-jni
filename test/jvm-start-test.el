;;; jvm-start-test.el --- Tests for jvm-start -*- lexical-binding: t; -*-

(require 'ert)
(require 'emacs-jni)

(ert-deftest emacs-jni-jvm-start-test/reaches-ready ()
  (should (eq (jvm-start-sync) 'ready)))

(provide 'jvm-start-test)
;;; jvm-start-test.el ends here
