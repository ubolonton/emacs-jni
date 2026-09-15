;;; jvm-call-test.el --- Tests for jvm-call/jvm-new -*- lexical-binding: t; -*-

(require 'ert)
(require 'jvm)

(defvar jvm-call-test--calc-jar
  (expand-file-name "../target/test-fixtures/calc.jar"
                     (file-name-directory (or load-file-name buffer-file-name))))

(defun jvm-call-test--ensure-ready ()
  "Ensure the JVM is running and the Calc fixture jar is loaded.
Each test calls this itself rather than relying on a previous test having
already done so: ERT's default `t' selector enumerates tests via
`apropos-internal' over the obarray, which does not guarantee definition
order (verified empirically — this file's tests do not reliably run in
the order they appear in when driven that way), so every test must be
independently able to reach a ready JVM with the fixture jar loaded."
  (unless (eq (jvm-status) 'ready)
    (should (eq (jvm-start-await) 'ready)))
  ;; Loading the same jar into the shared classloader more than once is
  ;; harmless (`URLClassLoader#addURL' just appends to its URL list), so
  ;; this is safe to call unconditionally from every test.
  (should (equal (jvm-load-jar-await jvm-call-test--calc-jar) '(t))))

(ert-deftest emacs-jni-jvm-call-test/static-call-and-instance-call ()
  (jvm-call-test--ensure-ready)
  (should (equal (jvm-call-await "Calc" "addLong" '(2 3)) '(t 5 nil)))
  (let ((obj (nth 1 (jvm-new-await "Calc" '(10)))))
    (should (user-ptrp obj))
    (should (equal (jvm-call-await obj "addToBase" '(5)) '(t 15 nil)))))

(ert-deftest emacs-jni-jvm-call-test/nil-marshals-to-false-for-boolean-param ()
  (jvm-call-test--ensure-ready)
  (should (equal (jvm-call-await "Calc" "describe" '(nil)) '(t "no" nil))))

(ert-deftest emacs-jni-jvm-call-test/bignum-round-trips ()
  (jvm-call-test--ensure-ready)
  (let ((big (* (expt 2 62) (expt 2 62)))) ; exceeds i64/long range, forces bignum
    (should (equal (jvm-call-await "Calc" "addBig" (list big 1)) (list t (1+ big) nil)))))

(ert-deftest emacs-jni-jvm-call-test/overload-resolution-falls-through-to-the-matching-candidate ()
  "Regression test for Finding 2 of the Phase 2 whole-branch review:
`Calc' declares two same-name, same-arity overloads --
`pick(long)' and `pick(String)' -- so `invoke_method' must try more than
just whichever candidate `getMethods()' happens to return first for a
given call to actually succeed, whichever it is."
  (jvm-call-test--ensure-ready)
  (should (equal (jvm-call-await "Calc" "pick" '("hello")) '(t "string" nil)))
  (should (equal (jvm-call-await "Calc" "pick" '(42)) '(t "long" nil))))

(ert-deftest emacs-jni-jvm-call-test/unsupported-argument-value-signals-instead-of-silently-becoming-t ()
  "A Lisp value with no marshalling rule (e.g. a vector) must signal
`jvm-invalid-argument' rather than silently being sent to Java as `true'."
  (jvm-call-test--ensure-ready)
  (should-error (jvm-call-await "Calc" "addLong" (list [1 2 3] 3))
                :type 'jvm-invalid-argument))

(provide 'jvm-call-test)
;;; jvm-call-test.el ends here
