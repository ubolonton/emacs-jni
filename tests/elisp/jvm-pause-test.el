;;; jvm-pause-test.el --- Tests for jvm-pause/resume -*- lexical-binding: t; -*-

(require 'ert)
(require 'jvm)

(ert-deftest emacs-jni-jvm-pause-test/pause-then-resume ()
  (should (eq (jvm-start-await) 'ready))
  (should (eq (jvm-pause-await) 'paused))
  (should (eq (jvm-start-await) 'ready)))

(ert-deftest emacs-jni-jvm-pause-test/status-not-stuck-after-illegal-transition ()
  "An illegal `jvm-pause' reverts `jvm--status' to the prior status."
  (unless (eq (jvm-status) 'ready)
    (should (eq (jvm-start-await) 'ready)))
  (should (eq (jvm-pause-await) 'paused))
  (should-error (jvm-pause-async) :type 'jvm-illegal-transition)
  (should (eq (jvm-status) 'paused)))

(provide 'jvm-pause-test)
;;; jvm-pause-test.el ends here
