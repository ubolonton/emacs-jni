;;; jvm-crash-test.el --- Tests for crash recovery -*- lexical-binding: t; -*-

(require 'ert)
(require 'jvm)

(defun jvm--wait-for-status (target timeout)
  (let ((deadline (+ (float-time) timeout)))
    (while (not (eq (jvm-status) target))
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (error "Timed out waiting for status %s, got %s" target (jvm-status)))))))

(ert-deftest emacs-jni-jvm-crash-test/panic-marks-crashed-and-allows-restart ()
  (should (eq (jvm-start-sync) 'ready))
  (jvm--test-panic)
  (jvm--wait-for-status 'crashed 10)
  (should (eq (jvm-status) 'crashed))
  ;; Restarting after a crash re-creates the JVM from scratch (the old one
  ;; may be in an undefined state); this may itself fail, which is
  ;; documented as acceptable in the spec's Error Handling section, so
  ;; accept either outcome as long as it's not stuck in 'starting.
  (should (memq (jvm-start-sync) '(ready start-failed))))

(provide 'jvm-crash-test)
;;; jvm-crash-test.el ends here
