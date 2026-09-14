;;; jvm-clj-eval-direct-test.el --- Tests for direct Clojure eval -*- lexical-binding: t; -*-

;; The synchronous, main-thread Clojure entry points
;; (`jvm-clj-eval-string-direct', `jvm-clj-require-direct'), built on
;; `jvm-call-direct' rather than the async worker. Unlike the async
;; `jvm-clj-eval-string' (jvm-clj-eval-test.el), every call runs on the
;; same, permanently-attached main thread, so a thread-local effect persists
;; across calls.

(require 'ert)
(require 'jvm-clj)

(defvar jvm-clj-eval-direct-test--jars-dir (getenv "EMACS_JNI_CLOJURE_JARS_DIR"))

(defvar jvm-clj-eval-direct-test--ready nil)

(defun jvm-clj-eval-direct-test--ensure-ready ()
  (unless (and jvm-clj-eval-direct-test--jars-dir
               (file-directory-p jvm-clj-eval-direct-test--jars-dir))
    (ert-skip "Set EMACS_JNI_CLOJURE_JARS_DIR (see test/fetch-clojure-jars.sh) to run this test"))
  (unless jvm-clj-eval-direct-test--ready
    (should (eq (jvm-start-sync) 'ready))
    (dolist (jar (directory-files jvm-clj-eval-direct-test--jars-dir t "\\.jar\\'"))
      (should (car (jvm-load-jar-sync jar))))
    (setq jvm-clj-eval-direct-test--ready t)))

(ert-deftest emacs-jni-jvm-clj-eval-direct-test/eval-returns-marshalled-scalar ()
  "A direct eval returns the result value itself, not an (OK RESULT ERR) tuple."
  (jvm-clj-eval-direct-test--ensure-ready)
  (should (equal (jvm-clj-eval-string-direct "(+ 40 (* 2 1))") 42)))

(ert-deftest emacs-jni-jvm-clj-eval-direct-test/require-direct-loads-a-namespace ()
  (jvm-clj-eval-direct-test--ensure-ready)
  ;; `require' returns nil on success; a bad namespace signals `jvm-error'.
  (should (null (jvm-clj-require-direct "clojure.set")))
  (should-error (jvm-clj-require-direct "no.such.namespace.exists")
                :type 'jvm-error))

(ert-deftest emacs-jni-jvm-clj-eval-direct-test/thread-local-persists-direct-but-not-async ()
  "Because direct evals share the one always-attached main thread, a
`ThreadLocal' value set by one direct eval is visible to the next. The async
`jvm-clj-eval-string' runs each call on a fresh worker-spawned thread, so
the same value is invisible to the following call (returns nil). The var
holding the `ThreadLocal' is global either way; only its per-thread *value*
differs."
  (jvm-clj-eval-direct-test--ensure-ready)
  ;; Direct: set on this thread, read back on the same thread → 42.
  (jvm-clj-eval-string-direct "(def tl-direct (java.lang.ThreadLocal.))")
  (jvm-clj-eval-string-direct "(.set tl-direct 42)")
  (should (equal (jvm-clj-eval-string-direct "(.get tl-direct)") 42))
  ;; Async: set on one worker thread, read on another → nil result.
  (jvm-clj-eval-string "(def tl-async (java.lang.ThreadLocal.))")
  (jvm-clj-eval-string "(.set tl-async 99)")
  (should (equal (jvm-clj-eval-string "(.get tl-async)") '(t nil nil))))

(ert-deftest emacs-jni-jvm-clj-eval-direct-test/nrepl-start-stop-roundtrip ()
  "The direct nREPL path starts a server on the main thread and stops it,
returning the (PORT . SERVER) pair directly -- not an (OK RESULT ERR) tuple
like `jvm-clj-nrepl-start'."
  (jvm-clj-eval-direct-test--ensure-ready)
  (let* ((result (jvm-clj-nrepl-start-direct))
         (port (car result))
         (server (cdr result)))
    (should (integerp port))
    (should (> port 0))
    (jvm-clj-nrepl-stop-direct server)))

(provide 'jvm-clj-eval-direct-test)
;;; jvm-clj-eval-direct-test.el ends here
