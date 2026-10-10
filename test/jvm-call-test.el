;;; jvm-call-test.el --- Tests for jvm-call/jvm-new -*- lexical-binding: t; -*-

(require 'ert)
(require 'jvm)

(defvar jvm-call-test--calc-jar
  (expand-file-name "../target/test-fixtures/calc.jar"
                     (file-name-directory (or load-file-name buffer-file-name))))

(defun jvm-call-test--ensure-ready ()
  "Ensure the JVM is running and the Calc fixture jar is loaded.
Every test calls this: ERT does not run tests in definition order."
  (unless (eq (jvm-status) 'ready)
    (should (eq (jvm-start-await) 'ready)))
  ;; Loading the same jar again is harmless.
  (jvm-load-jar-await jvm-call-test--calc-jar))

(ert-deftest emacs-jni-jvm-call-test/static-call-and-instance-call ()
  (jvm-call-test--ensure-ready)
  (should (equal (jvm-call-await "Calc" "addLong" '(2 3)) 5))
  (let ((obj (jvm-new-await "Calc" '(10))))
    (should (user-ptrp obj))
    (should (equal (jvm-call-await obj "addToBase" '(5)) 15))))

(ert-deftest emacs-jni-jvm-call-test/nil-marshals-to-false-for-boolean-param ()
  (jvm-call-test--ensure-ready)
  (should (equal (jvm-call-await "Calc" "describe" '(nil)) "no")))

(ert-deftest emacs-jni-jvm-call-test/bignum-round-trips ()
  (jvm-call-test--ensure-ready)
  (let ((big (* (expt 2 62) (expt 2 62)))) ; exceeds i64/long range, forces bignum
    (should (equal (jvm-call-await "Calc" "addBig" (list big 1)) (1+ big)))))

(ert-deftest emacs-jni-jvm-call-test/overload-resolution-falls-through-to-the-matching-candidate ()
  "`Calc' has `pick(long)' and `pick(String)'.
`invoke_method' must try the next candidate when ARGS do not fit."
  (jvm-call-test--ensure-ready)
  (should (equal (jvm-call-await "Calc" "pick" '("hello")) "string"))
  (should (equal (jvm-call-await "Calc" "pick" '(42)) "long")))

(ert-deftest emacs-jni-jvm-call-test/constructor-overload-falls-through-to-the-matching-candidate ()
  "`Calc' has `Calc(long)' and `Calc(String)'.
A string marshals for `Calc(long)' but `newInstance' rejects it, so the
fall-through must also handle an invoke-time mismatch."
  (jvm-call-test--ensure-ready)
  (let ((obj (jvm-new-await "Calc" '("hi"))))
    (should (user-ptrp obj))
    ;; Calc(String) sets base = label length (2); addToBase(3) = 5.
    (should (equal (jvm-call-await obj "addToBase" '(3)) 5))))

(ert-deftest emacs-jni-jvm-call-test/unsupported-argument-value-signals-instead-of-silently-becoming-t ()
  "A Lisp value with no marshalling rule (e.g. a vector) signals
`jvm-invalid-argument'."
  (jvm-call-test--ensure-ready)
  (should-error (jvm-call-await "Calc" "addLong" (list [1 2 3] 3))
                :type 'jvm-invalid-argument))

(ert-deftest emacs-jni-jvm-call-test/async-call-returns-keyword ()
  "The async call-done path returns a keyword."
  (jvm-call-test--ensure-ready)
  (should (eq (jvm-call-await "Calc" "callWithArg" (list (lambda (_n) :ok) 1)) :ok)))

(defun jvm-call-test--spin (n)
  "Loop N times with no I/O.
Unlike `sleep-for', this cannot run the process filter, so completions
that arrive meanwhile are read in one batch."
  (let ((i 0)) (while (< i n) (setq i (1+ i)))))

(ert-deftest emacs-jni-jvm-call-test/batch-fault-isolation-one-callback-error-does-not-drop-sibling ()
  "An error in one completion callback must not drop the next message.
`service_inbound' drains a batch of messages. A callback that signals must
not abort the rest of the batch; an error escaping a `:filter' kills
`emacs --batch'.

The two async calls and the spin make both completions land in one batch.
If they do not, the test still passes."
  (jvm-call-test--ensure-ready)
  (let (result2)
    (jvm-call-async "Calc" "addLong" '(1 1) (lambda (&rest _) (error "boom")))
    (jvm-call-async "Calc" "addLong" '(10 20)
                    (lambda (ok res _err) (when ok (setq result2 res))))
    (jvm-call-test--spin 300000)
    (let ((deadline (+ (float-time) 5)))
      (while (and (null result2) (< (float-time) deadline))
        (accept-process-output jvm--pipe-process 0.1)))
    (should (equal result2 30))))

(ert-deftest emacs-jni-jvm-call-test/distinct-objects-are-distinct-user-ptrs ()
  "Two JVM objects are distinct, non-`eq' `user-ptr's."
  (jvm-call-test--ensure-ready)
  (let ((a (jvm-new-await "Calc" '(1)))
        (b (jvm-new-await "Calc" '(2))))
    (should (user-ptrp a))
    (should (user-ptrp b))
    (should-not (eq a b))
    (should (equal (jvm-call-await a "addToBase" '(10)) 11))
    (should (equal (jvm-call-await b "addToBase" '(10)) 12))))

(provide 'jvm-call-test)
;;; jvm-call-test.el ends here
