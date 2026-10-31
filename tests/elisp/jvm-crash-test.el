;;; jvm-crash-test.el --- Tests for crash recovery -*- lexical-binding: t; -*-

(require 'ert)
(require 'jvm)

(declare-function jvm--test-panic "ext:emacs-jni" ())

(defun jvm--wait-for-status (target timeout)
  (let ((deadline (+ (float-time) timeout)))
    (while (not (eq (jvm-status) target))
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (error "Timed out waiting for status %s, got %s" target (jvm-status)))))))

(ert-deftest emacs-jni-jvm-crash-test/panic-marks-crashed-and-allows-restart ()
  ;; Only a dev build with `--features test-support' has `jvm--test-panic'.
  (skip-unless (fboundp 'jvm--test-panic))
  (should (eq (jvm-start-await) 'ready))
  (jvm--test-panic)
  (jvm--wait-for-status 'crashed 10)
  (should (eq (jvm-status) 'crashed))
  ;; A restart after a crash can fail; it must not stay at `starting'.
  (should (memq (jvm-start-await) '(ready start-failed))))

(provide 'jvm-crash-test)
;;; jvm-crash-test.el ends here
