;;; jvm-call-direct-test.el --- Tests for jvm-call-direct -*- lexical-binding: t; -*-

;; The synchronous, main-thread call path (`jvm--call-direct'): attach,
;; execute, return, with no worker round trip. Mirrors `jvm-call-test.el',
;; but the return shape is the value itself (not a `(OK RESULT ERR)' tuple)
;; and a failure signals `jvm-error' rather than filling in ERR.

(require 'ert)
(require 'jvm)

(defvar jvm-call-direct-test--calc-jar
  (expand-file-name "../target/test-fixtures/calc.jar"
                     (file-name-directory (or load-file-name buffer-file-name))))

(defun jvm-call-direct-test--ensure-ready ()
  "Ensure the JVM is running and the Calc fixture jar is loaded.
See `jvm-call-test--ensure-ready' for why every test does this itself."
  (unless (eq (jvm-status) 'ready)
    (should (eq (jvm-start-sync) 'ready)))
  (should (equal (jvm-load-jar-sync jvm-call-direct-test--calc-jar) '(t))))

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

(ert-deftest emacs-jni-jvm-call-direct-test/result-matches-jvm-call-sync ()
  "The direct value must match the RESULT the async blocking wrapper reports."
  (jvm-call-direct-test--ensure-ready)
  (should (equal (jvm-call-direct "Calc" "addLong" '(20 22))
                 (nth 1 (jvm-call-sync "Calc" "addLong" '(20 22))))))

(ert-deftest emacs-jni-jvm-call-direct-test/unsupported-argument-value-signals ()
  (jvm-call-direct-test--ensure-ready)
  (should-error (jvm-call-direct "Calc" "addLong" (list [1 2 3] 3))
                :type 'jvm-invalid-argument))

(ert-deftest emacs-jni-jvm-call-direct-test/missing-method-signals-jvm-error ()
  (jvm-call-direct-test--ensure-ready)
  (should-error (jvm-call-direct "Calc" "noSuchMethod" '())
                :type 'jvm-error))

(ert-deftest emacs-jni-jvm-call-direct-test/thread-identity-is-stable-across-calls ()
  "The main thread stays permanently attached across direct calls, so its
`java.lang.Thread' peer -- hence `Thread.currentThread().threadId()' -- is
identical for two separate top-level calls. (When each call detached the
thread, a fresh peer with a new id was minted every time.)"
  (jvm-call-direct-test--ensure-ready)
  (let ((id1 (jvm-call-direct
              (jvm-call-direct "java.lang.Thread" "currentThread" nil) "threadId" nil))
        (id2 (jvm-call-direct
              (jvm-call-direct "java.lang.Thread" "currentThread" nil) "threadId" nil)))
    (should (equal id1 id2))))

(provide 'jvm-call-direct-test)
;;; jvm-call-direct-test.el ends here
