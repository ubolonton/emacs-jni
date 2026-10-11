;;; dev-paths.el --- Use jvm and jvm-clj from a source checkout -*- lexical-binding: t; -*-

;;; Commentary:

;; Load this file before `jvm' to use the checkout's Lisp files, debug
;; module, and sources instead of an installed package:
;;
;;   emacs -Q -l dev-paths.el

;;; Code:

(let ((root (file-name-directory (or load-file-name buffer-file-name))))
  (add-to-list 'load-path (expand-file-name "elisp" root))
  (setq jvm-module-directory (expand-file-name "target/debug/" root)
        jvm--source-directory root))

;;; dev-paths.el ends here
