;;; jvm-invoke-test.el --- Tests for the JVM-calls-Lisp round trip -*- lexical-binding: t; -*-

(require 'ert)
(require 'jvm)

(defvar jvm-invoke-test--calc-jar
  (expand-file-name "../target/test-fixtures/calc.jar"
                     (file-name-directory (or load-file-name buffer-file-name))))

(ert-deftest emacs-jni-jvm-invoke-test/java-calls-lisp-and-gets-the-result-back ()
  (should (eq (jvm-start-await) 'ready))
  (jvm-load-jar-await jvm-invoke-test--calc-jar)
  (should (equal (jvm-call-await "Calc" "callWithArg" (list (lambda (n) (* n 2)) 21))
                 42)))

(ert-deftest emacs-jni-jvm-invoke-test/lisp-error-propagates-as-java-exception ()
  "A failed Lisp callback completes the future exceptionally with
`emacs_jni.core.LispNonLocalExit', so `Calc.callWithArg''s `future.get()'
raises it. Asserts on the exception TYPE, not message text: unlike the
old `LispError', `LispNonLocalExit' carries no plain-text message of its
own (its `dataOrValue' field holds \"boom\" instead, not introspectable
from this coarse exception-chain rendering)."
  (should (eq (jvm-status) 'ready))
  (let ((err (should-error (jvm-call-await "Calc" "callWithArg"
                                           (list (lambda (_n) (error "boom")) 1))
                           :type 'jvm-error)))
    (should (string-match-p "LispNonLocalExit" (cadr err)))))

(ert-deftest emacs-jni-jvm-invoke-test/java-calls-lisp-with-a-real-object-argument ()
  "Regression test for Finding 1 of the Phase 2 whole-branch review: a
JVM-initiated call whose argument is a real Java object (not a primitive)
must arrive at the Lisp callback as a genuine `user-ptr', not as the raw
wire tag `(jvm-handle N)'. Before the fix, `jvm--dispatch-message''s
`invoke' case applied the callback directly to the unresolved wire ARGS, so
OBJ below would have been the 2-element list `(jvm-handle N)' and
`user-ptrp' would have returned nil."
  (should (eq (jvm-status) 'ready))
  (jvm-load-jar-await jvm-invoke-test--calc-jar)
  (let ((obj (jvm-new-await "Calc" '(7))))
    (should (user-ptrp obj))
    (should (equal (jvm-call-await obj "callWithSelf"
                                   (list (lambda (received)
                                           (if (user-ptrp received) "yes" "no"))))
                   "yes"))))

(provide 'jvm-invoke-test)
;;; jvm-invoke-test.el ends here
