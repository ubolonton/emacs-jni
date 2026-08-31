;;; jvm-start-test.el --- Tests for jvm-start -*- lexical-binding: t; -*-

(require 'ert)
(require 'emacs-jni)

(ert-deftest emacs-jni-jvm-start-test/reaches-ready ()
  (should (eq (jvm-start-sync) 'ready)))

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

(provide 'jvm-start-test)
;;; jvm-start-test.el ends here
