;;; jvm-clj-eval-test.el --- Tests for Clojure eval + nREPL -*- lexical-binding: t; -*-

(require 'ert)
(require 'jvm-clj)

(defvar jvm-clj-eval-test--ready nil
  "Non-nil once the JVM is started and the jars are loaded.")

(defun jvm-clj-eval-test--ensure-ready ()
  (unless jvm-clj-eval-test--ready
    (jvm-clj-start-await)
    (setq jvm-clj-eval-test--ready t)))

(ert-deftest emacs-jni-jvm-clj-eval-test/context-classloader-fix-lets-clojure-bootstrap ()
  "`Clojure/var', `read', and `require' work on a worker thread.
These need the context classloader set on the thread; without it, Clojure
fails with `ExceptionInInitializerError'."
  (jvm-clj-eval-test--ensure-ready)
  (let* ((require-fn (jvm-call-await "clojure.java.api.Clojure" "var" (list "clojure.core" "require")))
         (sym (jvm-call-await "clojure.java.api.Clojure" "read" (list "clojure.set"))))
    (should (null (jvm-call-await require-fn "invoke" (list sym))))))

(ert-deftest emacs-jni-jvm-clj-eval-test/require-a-namespace ()
  (jvm-clj-eval-test--ensure-ready)
  (should (null (jvm-clj-require-await "clojure.set"))))

(ert-deftest emacs-jni-jvm-clj-eval-test/eval-string-returns-marshalled-scalar ()
  (jvm-clj-eval-test--ensure-ready)
  (should (equal (jvm-clj-eval-string-await "(+ 40 (* 2 1))") 42)))

(ert-deftest emacs-jni-jvm-clj-eval-test/eval-string-propagates-clojure-error ()
  (jvm-clj-eval-test--ensure-ready)
  (let ((err (should-error (jvm-clj-eval-string-await "(/ 1 0)") :type 'jvm-error)))
    (should (string-match-p "Divide by zero" (cadr err)))))

(ert-deftest emacs-jni-jvm-clj-nrepl-test/start-serves-real-bencode-protocol ()
  "An nREPL server answers a bencode `eval' from a separate process.
The client is bash with /dev/tcp, as an external client would be."
  (jvm-clj-eval-test--ensure-ready)
  (let* ((start-result (jvm-clj-nrepl-start-await))
         (port (car start-result))
         (server (cdr start-result))
         (code "(+ 40 (* 2 1))")
         (script (format
                  "exec 3<>/dev/tcp/127.0.0.1/%d; printf '%%s' 'd2:op4:eval4:code%d:%s2:id1:1e' >&3; timeout 5 cat <&3; exec 3<&-; exec 3>&-"
                  port (length code) code))
         (response (with-temp-buffer
                     (call-process "bash" nil t nil "-c" script)
                     (buffer-string))))
      ;; Bencode for the `value' key and the result "42".
      (should (string-match-p "5:value2:42" response))
      ;; FIX: No `unwind-protect', so a failing `should' leaves the server
      ;; running until the batch process exits.
      (jvm-clj-nrepl-stop-await server)))

(ert-deftest emacs-jni-jvm-clj-nrepl-test/bind-opens-the-server-beyond-loopback ()
  "With BIND, the server listens on that address, not loopback.
Asserts that the listen socket is a wildcard address."
  (jvm-clj-eval-test--ensure-ready)
  (unless (executable-find "ss")
    (ert-skip "needs `ss' to inspect the listening socket's bind address"))
  (let* ((start-result (jvm-clj-nrepl-start-await 0 "0.0.0.0"))
         (port (car start-result))
         (server (cdr start-result)))
    (unwind-protect
        (let ((listen (with-temp-buffer
                        (call-process "ss" nil t nil "-tlnH")
                        (buffer-string))))
          ;; How `ss' prints a wildcard bind.
          (should (string-match-p (format "\\(\\*\\|0\\.0\\.0\\.0\\|\\[::\\]\\):%d\\b" port) listen))
          (should-not (string-match-p (format "127\\.0\\.0\\.1:%d\\b" port) listen)))
      (jvm-clj-nrepl-stop-await server))))

(ert-deftest emacs-jni-jvm-clj-eval-test/eval-string-await-returns-keyword ()
  (jvm-clj-eval-test--ensure-ready)
  (should (eq (jvm-clj-eval-string-await ":a") :a)))

(provide 'jvm-clj-eval-test)
;;; jvm-clj-eval-test.el ends here
