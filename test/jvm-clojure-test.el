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

(ert-deftest emacs-jni-jvm-clojure-test/require-a-namespace ()
  (jvm-clojure-test--ensure-ready)
  (should (equal (jvm-clojure-require "clojure.set") '(t nil nil))))

(ert-deftest emacs-jni-jvm-clojure-test/eval-string-returns-marshalled-scalar ()
  (jvm-clojure-test--ensure-ready)
  (should (equal (jvm-clojure-eval-string "(+ 40 (* 2 1))") '(t 42 nil))))

(ert-deftest emacs-jni-jvm-clojure-test/eval-string-propagates-clojure-error ()
  (jvm-clojure-test--ensure-ready)
  (let ((result (jvm-clojure-eval-string "(/ 1 0)")))
    (should (eq (nth 0 result) nil))
    (should (string-match-p "Divide by zero" (nth 2 result)))))

(ert-deftest emacs-jni-jvm-nrepl-test/start-serves-real-bencode-protocol ()
  "Starts a real nREPL server and connects to it from a SEPARATE OS
process (bash + /dev/tcp) speaking the real bencode wire protocol -- this
is deliberately not Emacs's own network stack, since that's what an
actual external nREPL client does, and it's the exact mechanism verified
working during the Phase 3 design spike (see the spec's Feasibility
section)."
  (jvm-clojure-test--ensure-ready)
  (let ((start-result (jvm-nrepl-start)))
    (should (car start-result))
    (let* ((port (car (nth 1 start-result)))
           (server (cdr (nth 1 start-result)))
           (code "(+ 40 (* 2 1))")
           (script (format
                    "exec 3<>/dev/tcp/127.0.0.1/%d; printf '%%s' 'd2:op4:eval4:code%d:%s2:id1:1e' >&3; timeout 5 cat <&3; exec 3<&-; exec 3>&-"
                    port (length code) code))
           (response (with-temp-buffer
                       (call-process "bash" nil t nil "-c" script)
                       (buffer-string))))
      ;; Bencode has no quoting: a correct eval response contains the
      ;; literal substring "5:value2:42" (the "value" key's 5-char length
      ;; prefix, then the "42" result's 2-char length prefix) -- this is
      ;; the exact response shape observed during the design spike, not a
      ;; guess.
      (should (string-match-p "5:value2:42" response))
      ;; No `unwind-protect': a failing `should' above leaves this
      ;; server thread running for the rest of this batch process. Low
      ;; impact (the process exits right after), but a real gap if this
      ;; test is extended.
      (jvm-nrepl-stop server))))

(ert-deftest emacs-jni-jvm-nrepl-test/bind-opens-the-server-beyond-loopback ()
  "Passing BIND makes the server listen on the given interface instead
of nREPL's loopback default. A loopback-only server is unreachable
through VM/NAT port forwarding, which delivers the connection to a
routable interface, not to the guest's `127.0.0.1'. Asserts the LISTEN
socket for the bound port is a wildcard address, not loopback."
  (jvm-clojure-test--ensure-ready)
  (unless (executable-find "ss")
    (ert-skip "needs `ss' to inspect the listening socket's bind address"))
  (let ((start-result (jvm-nrepl-start 0 "0.0.0.0")))
    (should (car start-result))
    (let ((port (car (nth 1 start-result)))
          (server (cdr (nth 1 start-result))))
      (unwind-protect
          (let ((listen (with-temp-buffer
                          (call-process "ss" nil t nil "-tlnH")
                          (buffer-string))))
            ;; Bound to a wildcard address (`*'/`0.0.0.0'/`[::]'), the
            ;; forms `ss' prints for an all-interfaces bind -- verified
            ;; empirically against nREPL 1.7.0.
            (should (string-match-p (format "\\(\\*\\|0\\.0\\.0\\.0\\|\\[::\\]\\):%d\\b" port) listen))
            ;; ...and specifically NOT loopback-only, which is what the
            ;; default (no BIND) produces and what this option fixes.
            (should-not (string-match-p (format "127\\.0\\.0\\.1:%d\\b" port) listen)))
        (jvm-nrepl-stop server)))))

(provide 'jvm-clojure-test)
;;; jvm-clojure-test.el ends here
