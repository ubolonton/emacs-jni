;;; emacs-jni.el --- JVM embedding for Emacs -*- lexical-binding: t; -*-

(defvar emacs-jni--module-path
  (or (getenv "EMACS_JNI_MODULE_PATH")
      (expand-file-name "../target/debug/libemacs_jni.so"
                         (file-name-directory (or load-file-name buffer-file-name))))
  "Path to the built emacs-jni dynamic module (.so file).")

(unless (featurep 'emacs-jni)
  (module-load emacs-jni--module-path))

(provide 'emacs-jni)
;;; emacs-jni.el ends here
