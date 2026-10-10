;;; jvm-stop-test.el --- Tests for jvm-stop -*- lexical-binding: t; -*-

(require 'ert)
(require 'jvm)

(ert-deftest emacs-jni-jvm-stop-test/stop-from-ready ()
  (should (eq (jvm-start-await) 'ready))
  (should (memq (jvm-stop-await) '(stopped stop-failed))))

(ert-deftest emacs-jni-jvm-stop-test/stop-from-paused ()
  (should (eq (jvm-start-await) 'ready))
  (should (eq (jvm-pause-await) 'paused))
  (should (memq (jvm-stop-await) '(stopped stop-failed))))

(ert-deftest emacs-jni-jvm-stop-test/status-not-stuck-after-illegal-transition ()
  "An illegal `jvm-stop' reverts `jvm--status' to the prior status."
  (unless (memq (jvm-status) '(ready paused))
    (should (eq (jvm-start-await) 'ready)))
  (should (memq (jvm-stop-await) '(stopped stop-failed)))
  (should-error (jvm-stop-async) :type 'jvm-illegal-transition)
  (should (memq (jvm-status) '(stopped stop-failed))))

(provide 'jvm-stop-test)
;;; jvm-stop-test.el ends here
