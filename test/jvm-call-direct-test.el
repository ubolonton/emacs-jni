;;; jvm-call-direct-test.el --- Tests for jvm-call-direct -*- lexical-binding: t; -*-

;; The synchronous main-thread call path. Same cases as `jvm-call-test.el'.

(require 'ert)
(require 'jvm)

(defvar jvm-call-direct-test--calc-jar
  (expand-file-name "../target/test-fixtures/calc.jar"
                     (file-name-directory (or load-file-name buffer-file-name))))

(defun jvm-call-direct-test--ensure-ready ()
  "Ensure the JVM is running and the Calc fixture jar is loaded.
Every test calls this: ERT does not run tests in definition order."
  (unless (eq (jvm-status) 'ready)
    (should (eq (jvm-start-await) 'ready)))
  (jvm-load-jar-await jvm-call-direct-test--calc-jar))

(ert-deftest emacs-jni-jvm-call-direct-test/static-call-and-instance-call ()
  (jvm-call-direct-test--ensure-ready)
  (should (equal (jvm-call-direct "Calc" "addLong" '(2 3)) 5))
  (let ((obj (jvm-new-direct "Calc" '(10))))
    (should (user-ptrp obj))
    (should (equal (jvm-call-direct obj "addToBase" '(5)) 15))))

(ert-deftest emacs-jni-jvm-call-direct-test/nil-marshals-to-false-for-boolean-param ()
  (jvm-call-direct-test--ensure-ready)
  (should (equal (jvm-call-direct "Calc" "describe" '(nil)) "no")))

(ert-deftest emacs-jni-jvm-call-direct-test/bignum-round-trips ()
  (jvm-call-direct-test--ensure-ready)
  (let ((big (* (expt 2 62) (expt 2 62)))) ; exceeds i64/long range, forces bignum
    (should (equal (jvm-call-direct "Calc" "addBig" (list big 1)) (1+ big)))))

(ert-deftest emacs-jni-jvm-call-direct-test/overload-resolution-falls-through-to-the-matching-candidate ()
  (jvm-call-direct-test--ensure-ready)
  (should (equal (jvm-call-direct "Calc" "pick" '("hello")) "string"))
  (should (equal (jvm-call-direct "Calc" "pick" '(42)) "long")))

(ert-deftest emacs-jni-jvm-call-direct-test/result-matches-jvm-call-await ()
  "The direct value must match the RESULT the async blocking wrapper reports."
  (jvm-call-direct-test--ensure-ready)
  (should (equal (jvm-call-direct "Calc" "addLong" '(20 22))
                 (jvm-call-await "Calc" "addLong" '(20 22)))))

(ert-deftest emacs-jni-jvm-call-direct-test/unsupported-argument-value-signals ()
  (jvm-call-direct-test--ensure-ready)
  (should-error (jvm-call-direct "Calc" "addLong" (list [1 2 3] 3))
                :type 'jvm-invalid-argument))

(ert-deftest emacs-jni-jvm-call-direct-test/missing-method-signals-jvm-error ()
  (jvm-call-direct-test--ensure-ready)
  (should-error (jvm-call-direct "Calc" "noSuchMethod" '())
                :type 'jvm-error))

(ert-deftest emacs-jni-jvm-call-direct-test/thread-identity-is-stable-across-calls ()
  "The main thread stays attached, so `Thread.currentThread().threadId()'
is the same for two separate calls."
  (jvm-call-direct-test--ensure-ready)
  (let ((id1 (jvm-call-direct
              (jvm-call-direct "java.lang.Thread" "currentThread" nil) "threadId" nil))
        (id2 (jvm-call-direct
              (jvm-call-direct "java.lang.Thread" "currentThread" nil) "threadId" nil)))
    (should (equal id1 id2))))

(ert-deftest emacs-jni-jvm-call-direct-test/java-waits-on-a-lisp-callback ()
  "Java can wait on a Lisp callback's result inside a direct call."
  (jvm-call-direct-test--ensure-ready)
  (should (equal (jvm-call-direct "Calc" "callWithArg" (list (lambda (n) (* n 10)) 4))
                 40))
  (should (eq (jvm-call-direct "Calc" "callWithArg" (list (lambda (_n) :ok) 1)) :ok)))

(ert-deftest emacs-jni-jvm-call-direct-test/lisp-callback-error-reaches-java ()
  "A Lisp error in a callback reaches Java as a `LispSignal' inside a direct call."
  (jvm-call-direct-test--ensure-ready)
  (should (equal (jvm-call-direct "Calc" "describeCallbackError"
                                  (list (lambda (_n) (signal 'wrong-type-argument '(numberp "x")))
                                        1))
                 "wrong-type-argument")))

(ert-deftest emacs-jni-jvm-call-direct-test/callback-receives-a-java-object ()
  "A Java object passed to a callback inside a direct call arrives wrapped."
  (jvm-call-direct-test--ensure-ready)
  (let ((calc (jvm-new-direct "Calc" '(100))))
    (should (equal (jvm-call-direct calc "callWithSelf"
                                    (list (lambda (self) (jvm-call-direct self "addToBase" '(1)))))
                   101))))

(provide 'jvm-call-direct-test)
;;; jvm-call-direct-test.el ends here
