;;; smoke-test.el --- Install the jvm archive and smoke-test it -*- lexical-binding: t; -*-

;;; Commentary:

;; Usage: emacs -Q --batch -l test/package/smoke-test.el TAR
;;
;; Install TAR into a scratch `package-user-dir' next to TAR's directory,
;; then start the JVM, make one Java call, and evaluate one Clojure form.
;; Exit non-zero on failure.

;;; Code:

(require 'package)

(let* ((tar (expand-file-name (car command-line-args-left)))
       (parent (file-name-directory
                (directory-file-name (file-name-directory tar)))))
  (setq package-user-dir (expand-file-name "package-test-elpa" parent))
  (when (file-directory-p package-user-dir)
    (delete-directory package-user-dir t))
  (package-initialize)
  (package-install-file tar)
  (require 'jvm-clj)
  (unless (eq (jvm-start) 'ready)
    (error "JVM status: %s" (jvm-status)))
  (unless (equal (jvm-call "java.lang.Integer" "toHexString" '(255)) "ff")
    (error "jvm-call returned a wrong value"))
  (jvm-clj-start)
  (unless (equal (jvm-clj-eval-string "(+ 1 2)") 3)
    (error "jvm-clj-eval-string returned a wrong value"))
  (message "package-test: OK"))

;;; smoke-test.el ends here
