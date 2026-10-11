;;; jvm-clj-eval-direct-test.el --- Tests for direct Clojure eval -*- lexical-binding: t; -*-

;; Direct Clojure eval on the Emacs main thread. Unlike the async path,
;; every call runs on the same JVM thread, so Java `ThreadLocal' values
;; persist.

(require 'ert)
(require 'jvm-clj)

(defvar jvm-clj-eval-direct-test--ready nil)

(defun jvm-clj-eval-direct-test--ensure-ready ()
  (unless jvm-clj-eval-direct-test--ready
    (should (eq (jvm-start-await) 'ready))
    (dolist (jar (jvm-clj-ensure-jars))
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
  "A `ThreadLocal' value persists across direct evals, not async ones.
Each async eval runs on a new thread."
  (jvm-clj-eval-direct-test--ensure-ready)
  ;; Direct: same thread, so 42.
  (jvm-clj-eval-string-direct "(def tl-direct (java.lang.ThreadLocal.))")
  (jvm-clj-eval-string-direct "(.set tl-direct 42)")
  (should (equal (jvm-clj-eval-string-direct "(.get tl-direct)") 42))
  ;; Async: different threads, so nil.
  (jvm-clj-eval-string-await "(def tl-async (java.lang.ThreadLocal.))")
  (jvm-clj-eval-string-await "(.set tl-async 99)")
  (should (null (jvm-clj-eval-string-await "(.get tl-async)"))))

(ert-deftest emacs-jni-jvm-clj-eval-direct-test/nrepl-start-stop-roundtrip ()
  "The direct nREPL path starts and stops a server."
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
  "A Clojure symbol result returns as a Lisp symbol. The `'' reader macro works."
  (jvm-clj-eval-direct-test--ensure-ready)
  (should (eq (jvm-clj-eval-string-direct "'foo") 'foo)))

(ert-deftest emacs-jni-jvm-clj-eval-direct-test/eval-accepts-reader-macros ()
  "Other reader macros work: `#(...)' and `#\"re\"'."
  (jvm-clj-eval-direct-test--ensure-ready)
  (should (equal (jvm-clj-eval-string-direct "(#(+ % 1) 41)") 42))
  (should (equal (jvm-clj-eval-string-direct "(re-find #\"\\d+\" \"ab12cd\")") "12")))

(provide 'jvm-clj-eval-direct-test)
;;; jvm-clj-eval-direct-test.el ends here
