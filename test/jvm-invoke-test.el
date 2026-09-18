;;; jvm-invoke-test.el --- Tests for the JVM-calls-Lisp round trip -*- lexical-binding: t; -*-

(require 'ert)
(require 'jvm)

(defvar jvm-invoke-test--calc-jar
  (expand-file-name "../target/test-fixtures/calc.jar"
                     (file-name-directory (or load-file-name buffer-file-name))))

(ert-deftest emacs-jni-jvm-invoke-test/java-calls-lisp-and-gets-the-result-back ()
  ;; ERT's `t' selector enumerates tests via `apropos-internal' over the
  ;; obarray, sorted alphabetically -- not definition order (see
  ;; `jvm-call-test--ensure-ready' for the same note) -- so this cannot
  ;; assume it is the first test to touch the shared JVM singleton.
  (unless (eq (jvm-status) 'ready)
    (should (eq (jvm-start-await) 'ready)))
  (jvm-load-jar-await jvm-invoke-test--calc-jar)
  (should (equal (jvm-call-await "Calc" "callWithArg" (list (lambda (n) (* n 2)) 21))
                 42)))

(ert-deftest emacs-jni-jvm-invoke-test/lisp-error-propagates-as-lisp-signal ()
  "A failed Lisp callback completes the future exceptionally with a real
`emacs_jni.core.LispSignal' carrying the callback's actual `condition-case'
signal symbol -- not a fabricated `error' symbol with a stringified
message. `Calc.describeCallbackError' catches the future's exception and
returns the `LispSignal''s real symbol name, so this asserts on the real
symbol, not on coarse exception-chain text: before this task, every failed
callback crossed as a fabricated `(error . \"boom-string\")', so this would
return \"error\" regardless of what the callback actually signalled."
  (should (eq (jvm-status) 'ready))
  (jvm-load-jar-await jvm-invoke-test--calc-jar)
  (should (equal (jvm-call-await "Calc" "describeCallbackError"
                                 (list (lambda (_n) (signal 'wrong-type-argument '(numberp "x"))) 1))
                 "wrong-type-argument")))

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

(ert-deftest emacs-jni-jvm-invoke-test/callback-returning-keyword-symbol-function ()
  "A JVM->Lisp callback may return a keyword, a symbol, or a function.
Before Item 1 the invoke-reply path used a trimmed converter missing those
arms, so a keyword return signalled `jvm-invalid-argument' and surfaced as a
`jvm-error'."
  (unless (eq (jvm-status) 'ready)
    (should (eq (jvm-start-await) 'ready)))
  (jvm-load-jar-await jvm-invoke-test--calc-jar)
  ;; Keyword and symbol round-trip as themselves.
  (should (eq (jvm-call-await "Calc" "callWithArg" (list (lambda (_n) :a) 1)) :a))
  (should (eq (jvm-call-await "Calc" "callWithArg" (list (lambda (_n) 'foo) 1)) 'foo))
  ;; A function return marshals as a Callback -> LispFunction on the JVM,
  ;; which crosses back as an opaque handle (a user-ptr), not an error.
  (should (user-ptrp (jvm-call-await "Calc" "callWithArg"
                                     (list (lambda (_n) (lambda () 1)) 1)))))

(provide 'jvm-invoke-test)
;;; jvm-invoke-test.el ends here
