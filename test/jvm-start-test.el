;;; jvm-start-test.el --- Tests for jvm-start -*- lexical-binding: t; -*-

(require 'ert)
(require 'jvm)

(ert-deftest emacs-jni-jvm-start-test/reaches-ready ()
  (should (eq (jvm-start-sync) 'ready)))

(ert-deftest emacs-jni-jvm-start-test/default-options-include-xrs ()
  "`jvm-options' must default to include \"-Xrs\", which stops the JVM from
installing its own SIGINT/SIGTERM/SIGHUP/SIGQUIT handlers — without it,
since the JVM shares Emacs's process, `C-g' can terminate Emacs outright
instead of just interrupting it (empirically verified with a `sigaction'
probe during development; see docs/explanation/architecture.md). This
only guards the default Elisp value doesn't silently regress — it
doesn't re-verify the OS-level signal-handler effect itself, which
isn't practical to assert from Elisp."
  (should (member "-Xrs" jvm-options)))

(ert-deftest emacs-jni-jvm-start-test/status-not-stuck-after-illegal-transition ()
  "An illegal `jvm-start' while already `ready' must not leave
`jvm--status' stuck at `starting' — it should revert to the prior status
after the primitive signals `jvm-illegal-transition'.
`jvm-start-sync' is only called here if the JVM (a process-wide, one-time
resource shared with other tests in this same batch Emacs session) isn't
already `ready', since a second `jvm-start-sync' call once `ready' would
itself hit the same illegal-transition signal this test provokes on
purpose below."
  (unless (eq (jvm-status) 'ready)
    (should (eq (jvm-start-sync) 'ready)))
  (should-error (jvm-start) :type 'jvm-illegal-transition)
  (should (eq (jvm-status) 'ready)))

(ert-deftest emacs-jni-jvm-start-test/start-failed-message-reaches-log-buffer ()
  "A `(status start-failed MSG)' channel message must not silently drop MSG
\(Fix 1 of the final review\): it should be appended to the `*jvm-log*'
buffer. This drives `jvm--dispatch-message' directly with a synthetic
message rather than provoking a real `start-failed', since there's no
reliable way to make `JavaVM::new' fail fast from Elisp.
`jvm--status' is saved/restored around the synthetic dispatch so this
doesn't leak a fake `start-failed' status into the other tests sharing
this batch process."
  (when (get-buffer jvm--log-buffer-name)
    (kill-buffer jvm--log-buffer-name))
  (let ((previous-status jvm--status))
    (unwind-protect
        (progn
          (jvm--dispatch-message '(status start-failed "synthetic failure for test"))
          (with-current-buffer (get-buffer-create jvm--log-buffer-name)
            (should (string-match-p "synthetic failure for test" (buffer-string)))))
      (setq jvm--status previous-status))))

(provide 'jvm-start-test)
;;; jvm-start-test.el ends here
