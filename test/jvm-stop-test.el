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
  "An illegal `jvm-stop' while already `stopped'/`stop-failed' must not
leave `jvm--status' stuck at `stopping' — it should revert to the prior
status after the primitive signals `jvm-illegal-transition'.
Forces the JVM to `ready' (or reuses `paused', if that's where another
test in this process left it) and then to `stopped'/`stop-failed' itself
first, rather than relying on another test having already done so, so
this test is deterministic regardless of ERT's test ordering within this
process."
  (unless (memq (jvm-status) '(ready paused))
    (should (eq (jvm-start-await) 'ready)))
  (should (memq (jvm-stop-await) '(stopped stop-failed)))
  (should-error (jvm-stop-async) :type 'jvm-illegal-transition)
  (should (memq (jvm-status) '(stopped stop-failed))))

(provide 'jvm-stop-test)
;;; jvm-stop-test.el ends here
