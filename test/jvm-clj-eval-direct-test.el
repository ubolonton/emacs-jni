;;; jvm-clj-eval-direct-test.el --- Tests for direct Clojure eval -*- lexical-binding: t; -*-

;; The synchronous, main-thread Clojure entry points
;; (`jvm-clj-eval-string-direct', `jvm-clj-require-direct'), built on
;; `jvm-call-direct' rather than the async worker. Unlike the async
;; `jvm-clj-eval-string-await' (jvm-clj-eval-test.el), every call runs on the
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
    (should (eq (jvm-start-await) 'ready))
    (dolist (jar (directory-files jvm-clj-eval-direct-test--jars-dir t "\\.jar\\'"))
      (jvm-load-jar-await jar))
    (jvm-load-jar-await jvm-clj--source-dir)
    (jvm-clj-require-await "emacs-jni.clj")
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
`jvm-clj-eval-string-await' runs each call on a fresh worker-spawned thread, so
the same value is invisible to the following call (returns nil). The var
holding the `ThreadLocal' is global either way; only its per-thread *value*
differs."
  (jvm-clj-eval-direct-test--ensure-ready)
  ;; Direct: set on this thread, read back on the same thread → 42.
  (jvm-clj-eval-string-direct "(def tl-direct (java.lang.ThreadLocal.))")
  (jvm-clj-eval-string-direct "(.set tl-direct 42)")
  (should (equal (jvm-clj-eval-string-direct "(.get tl-direct)") 42))
  ;; Async: set on one worker thread, read on another → nil result.
  (jvm-clj-eval-string-await "(def tl-async (java.lang.ThreadLocal.))")
  (jvm-clj-eval-string-await "(.set tl-async 99)")
  (should (null (jvm-clj-eval-string-await "(.get tl-async)"))))

(ert-deftest emacs-jni-jvm-clj-eval-direct-test/nrepl-start-stop-roundtrip ()
  "The direct nREPL path starts a server on the main thread and stops it,
returning the (PORT . SERVER) pair directly -- not an (OK RESULT ERR) tuple
like `jvm-clj-nrepl-start-await'."
  (jvm-clj-eval-direct-test--ensure-ready)
  (let* ((result (jvm-clj-nrepl-start-direct))
         (port (car result))
         (server (cdr result)))
    (should (integerp port))
    (should (> port 0))
    (jvm-clj-nrepl-stop-direct server)))

(ert-deftest emacs-jni-jvm-clj-eval-direct-test/eval-returns-keyword ()
  "A Clojure keyword result comes back as a real Lisp keyword, not a pointer."
  (jvm-clj-eval-direct-test--ensure-ready)
  (should (eq (jvm-clj-eval-string-direct ":a") :a)))

(ert-deftest emacs-jni-jvm-clj-eval-direct-test/eval-returns-symbol ()
  "A Clojure symbol result comes back as a real Lisp symbol. The `'' shorthand
must work: eval-string reads with the full Clojure reader (`load-string'), not
EDN, so `'foo' reads as `(quote foo)'."
  (jvm-clj-eval-direct-test--ensure-ready)
  (should (eq (jvm-clj-eval-string-direct "'foo") 'foo)))

(ert-deftest emacs-jni-jvm-clj-eval-direct-test/eval-accepts-reader-macros ()
  "Reader macros beyond quote work too (EDN would reject these): an anonymous-fn
literal `#(...)' evaluates and applies, and a regex literal `#\"re\"' matches."
  (jvm-clj-eval-direct-test--ensure-ready)
  (should (equal (jvm-clj-eval-string-direct "(#(+ % 1) 41)") 42))
  (should (equal (jvm-clj-eval-string-direct "(re-find #\"\\d+\" \"ab12cd\")") "12")))

(provide 'jvm-clj-eval-direct-test)
;;; jvm-clj-eval-direct-test.el ends here
