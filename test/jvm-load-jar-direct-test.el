;;; jvm-load-jar-direct-test.el --- Tests for jvm-load-jar-direct -*- lexical-binding: t; -*-

;; The synchronous, main-thread jar-load path (`jvm--load-jar-direct'): add a
;; jar to the shared classloader on this thread, with no worker round trip.
;; Mirrors `jvm-load-jar-test.el', but returns nil / signals `jvm-error'
;; instead of a `(OK . ERR)' cons. The shared classloader is one object for
;; both the direct and the async paths, so a jar loaded either way resolves
;; from the other.

(require 'ert)
(require 'jvm)

(defvar jvm-load-jar-direct-test--calc-jar
  (expand-file-name "../target/test-fixtures/calc.jar"
                     (file-name-directory (or load-file-name buffer-file-name))))

(defun jvm-load-jar-direct-test--ensure-ready ()
  (unless (eq (jvm-status) 'ready)
    (should (eq (jvm-start-await) 'ready))))

(ert-deftest emacs-jni-jvm-load-jar-direct-test/loads-then-calls ()
  "A jar loaded on the direct path is resolvable by a direct call."
  (jvm-load-jar-direct-test--ensure-ready)
  (should (null (jvm-load-jar-direct jvm-load-jar-direct-test--calc-jar)))
  (should (equal (jvm-call-direct "Calc" "addLong" '(2 3)) 5)))

(ert-deftest emacs-jni-jvm-load-jar-direct-test/visible-to-async-path ()
  "The direct and async paths share one classloader: a jar loaded directly
resolves from an async `jvm-call-await' too."
  (jvm-load-jar-direct-test--ensure-ready)
  (should (null (jvm-load-jar-direct jvm-load-jar-direct-test--calc-jar)))
  (should (equal (jvm-call-await "Calc" "addLong" '(7 8)) 15)))

(ert-deftest emacs-jni-jvm-load-jar-direct-test/missing-jar-signals-jvm-error ()
  (jvm-load-jar-direct-test--ensure-ready)
  (should-error (jvm-load-jar-direct "/nonexistent/path/does-not-exist.jar")
                :type 'jvm-error))

(provide 'jvm-load-jar-direct-test)
;;; jvm-load-jar-direct-test.el ends here
