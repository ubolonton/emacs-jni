;;; jvm-clojure-direct-test.el --- Tests for direct Clojure eval -*- lexical-binding: t; -*-

;; The synchronous, main-thread Clojure entry points
;; (`jvm-clojure-eval-string-direct', `jvm-clojure-require-direct'), built on
;; `jvm-call-direct' rather than the async worker. Unlike the async
;; `jvm-clojure-eval-string' (jvm-clojure-test.el), every call runs on the
;; same, permanently-attached main thread, so a thread-local effect persists
;; across calls.

(require 'ert)
(require 'jvm)

(defvar jvm-clojure-direct-test--jars-dir (getenv "EMACS_JNI_CLOJURE_JARS_DIR"))

(defvar jvm-clojure-direct-test--ready nil)

(defun jvm-clojure-direct-test--ensure-ready ()
  (unless (and jvm-clojure-direct-test--jars-dir
               (file-directory-p jvm-clojure-direct-test--jars-dir))
    (ert-skip "Set EMACS_JNI_CLOJURE_JARS_DIR (see test/fetch-clojure-jars.sh) to run this test"))
  (unless jvm-clojure-direct-test--ready
    (should (eq (jvm-start-sync) 'ready))
    (dolist (jar (directory-files jvm-clojure-direct-test--jars-dir t "\\.jar\\'"))
      (should (car (jvm-load-jar-sync jar))))
    (setq jvm-clojure-direct-test--ready t)))

(ert-deftest emacs-jni-jvm-clojure-direct-test/eval-returns-marshalled-scalar ()
  "A direct eval returns the result value itself, not an (OK RESULT ERR) tuple."
  (jvm-clojure-direct-test--ensure-ready)
  (should (equal (jvm-clojure-eval-string-direct "(+ 40 (* 2 1))") 42)))

(ert-deftest emacs-jni-jvm-clojure-direct-test/require-direct-loads-a-namespace ()
  (jvm-clojure-direct-test--ensure-ready)
  ;; `require' returns nil on success; a bad namespace signals `jvm-error'.
  (should (null (jvm-clojure-require-direct "clojure.set")))
  (should-error (jvm-clojure-require-direct "no.such.namespace.exists")
                :type 'jvm-error))

(ert-deftest emacs-jni-jvm-clojure-direct-test/thread-local-persists-direct-but-not-async ()
  "Because direct evals share the one always-attached main thread, a
`ThreadLocal' value set by one direct eval is visible to the next. The async
`jvm-clojure-eval-string' runs each call on a fresh worker-spawned thread, so
the same value is invisible to the following call (returns nil). The var
holding the `ThreadLocal' is global either way; only its per-thread *value*
differs."
  (jvm-clojure-direct-test--ensure-ready)
  ;; Direct: set on this thread, read back on the same thread → 42.
  (jvm-clojure-eval-string-direct "(def tl-direct (java.lang.ThreadLocal.))")
  (jvm-clojure-eval-string-direct "(.set tl-direct 42)")
  (should (equal (jvm-clojure-eval-string-direct "(.get tl-direct)") 42))
  ;; Async: set on one worker thread, read on another → nil result.
  (jvm-clojure-eval-string "(def tl-async (java.lang.ThreadLocal.))")
  (jvm-clojure-eval-string "(.set tl-async 99)")
  (should (equal (jvm-clojure-eval-string "(.get tl-async)") '(t nil nil))))

(provide 'jvm-clojure-direct-test)
;;; jvm-clojure-direct-test.el ends here
