;;; jvm-clojure-test.el --- Tests for Clojure eval + nREPL -*- lexical-binding: t; -*-

(require 'ert)
(require 'emacs-jni)

(defvar jvm-clojure-test--jars-dir (getenv "EMACS_JNI_CLOJURE_JARS_DIR")
  "Directory containing Clojure/nREPL jars, or nil if unset.
Set via `test/fetch-clojure-jars.sh' and the EMACS_JNI_CLOJURE_JARS_DIR
environment variable — tests in this file skip cleanly, not fail, when
this isn't configured, so the suite stays runnable with no network
access.")

(defvar jvm-clojure-test--ready nil
  "Set to t once the JVM is started and jars are loaded, so every test
in this file can call `jvm-clojure-test--ensure-ready' idempotently
regardless of ERT's (non-definition-order) execution order — the exact
problem Phase 2's Task 5 already hit for `jvm-call-test.el'.")

(defun jvm-clojure-test--skip-unless-jars ()
  (unless (and jvm-clojure-test--jars-dir (file-directory-p jvm-clojure-test--jars-dir))
    (ert-skip "Set EMACS_JNI_CLOJURE_JARS_DIR (see test/fetch-clojure-jars.sh) to run this test")))

(defun jvm-clojure-test--ensure-ready ()
  (jvm-clojure-test--skip-unless-jars)
  (unless jvm-clojure-test--ready
    (should (eq (jvm-start-sync) 'ready))
    (dolist (jar (directory-files jvm-clojure-test--jars-dir t "\\.jar\\'"))
      (should (car (jvm-load-jar-sync jar))))
    (setq jvm-clojure-test--ready t)))

(ert-deftest emacs-jni-jvm-clojure-test/context-classloader-fix-lets-clojure-bootstrap ()
  "Proves Task 1's fix: without it, this fails with
ExceptionInInitializerError the first time Clojure's runtime is touched
\(see the spec's Feasibility section for the exact failure)."
  (jvm-clojure-test--ensure-ready)
  (let ((require-result (jvm-call-sync "clojure.java.api.Clojure" "var" (list "clojure.core" "require"))))
    (should (eq (nth 0 require-result) t))
    (let* ((require-fn (nth 1 require-result))
           (sym-result (jvm-call-sync "clojure.java.api.Clojure" "read" (list "clojure.set"))))
      (should (eq (nth 0 sym-result) t))
      (should (equal (jvm-call-sync require-fn "invoke" (list (nth 1 sym-result))) '(t nil nil))))))

(provide 'jvm-clojure-test)
;;; jvm-clojure-test.el ends here
