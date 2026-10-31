;;; smoke-test.el --- Smoke test for module loading -*- lexical-binding: t; -*-

(require 'ert)
(require 'jvm)

(ert-deftest emacs-jni-smoke-test/module-loads-and-responds ()
  (should (eq (jvm-ping) t)))

(provide 'smoke-test)
;;; smoke-test.el ends here
