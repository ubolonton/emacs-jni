;;; jvm-pause-test.el --- Tests for jvm-pause/resume -*- lexical-binding: t; -*-

(require 'ert)
(require 'jvm)

(ert-deftest emacs-jni-jvm-pause-test/pause-then-resume ()
  (should (eq (jvm-start-sync) 'ready))
  (should (eq (jvm-pause-sync) 'paused))
  (should (eq (jvm-start-sync) 'ready)))

(ert-deftest emacs-jni-jvm-pause-test/status-not-stuck-after-illegal-transition ()
  "An illegal `jvm-pause' while `paused' must not leave `jvm--status'
stuck at `pausing' — it should revert to the prior status after the
primitive signals `jvm-illegal-transition'.
Forces the JVM to `ready' then `paused' first (rather than relying on
another test file having already put it there), so this test is
deterministic regardless of ERT's test ordering within this process."
  (unless (eq (jvm-status) 'ready)
    (should (eq (jvm-start-sync) 'ready)))
  (should (eq (jvm-pause-sync) 'paused))
  (should-error (jvm-pause) :type 'jvm-illegal-transition)
  (should (eq (jvm-status) 'paused)))

(provide 'jvm-pause-test)
;;; jvm-pause-test.el ends here
