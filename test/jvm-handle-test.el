;;; jvm-handle-test.el --- Tests for the JVM-object user-ptr plumbing -*- lexical-binding: t; -*-

(require 'ert)
(require 'jvm)

(ert-deftest emacs-jni-jvm-handle-test/round-trips-as-a-distinct-lisp-value ()
  (let ((h1 (jvm--test-make-handle))
        (h2 (jvm--test-make-handle)))
    (should (user-ptrp h1))
    (should (user-ptrp h2))
    (should-not (eq h1 h2))))

(provide 'jvm-handle-test)
;;; jvm-handle-test.el ends here
