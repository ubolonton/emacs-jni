;;; jvm-start-test.el --- Tests for jvm-start -*- lexical-binding: t; -*-

(require 'ert)
(require 'jvm)

(ert-deftest emacs-jni-jvm-start-test/reaches-ready ()
  (should (eq (jvm-start-await) 'ready)))

(ert-deftest emacs-jni-jvm-start-test/default-options-include-xrs ()
  "`jvm-options' includes \"-Xrs\" by default.
See docs/explanation/architecture.md. This checks only the default value,
not the signal handlers."
  (should (member "-Xrs" jvm-options)))

(ert-deftest emacs-jni-jvm-start-test/status-not-stuck-after-illegal-transition ()
  "An illegal `jvm-start' reverts `jvm--status' to the prior status."
  (unless (eq (jvm-status) 'ready)
    (should (eq (jvm-start-await) 'ready)))
  (should-error (jvm-start-async) :type 'jvm-illegal-transition)
  (should (eq (jvm-status) 'ready)))

(ert-deftest emacs-jni-jvm-start-test/start-failed-message-reaches-log-buffer ()
  "A `start-failed' detail message goes to the `*jvm-log*' buffer.
Calls `jvm--apply-status' directly: a real start cannot be made to fail
from Elisp. Restores `jvm--status' for the other tests."
  (when (get-buffer jvm--log-buffer-name)
    (kill-buffer jvm--log-buffer-name))
  (let ((previous-status jvm--status))
    (unwind-protect
        (progn
          (jvm--apply-status "start-failed" "synthetic failure for test")
          (with-current-buffer (get-buffer-create jvm--log-buffer-name)
            (should (string-match-p "synthetic failure for test" (buffer-string)))))
      (setq jvm--status previous-status))))

(provide 'jvm-start-test)
;;; jvm-start-test.el ends here
