;;; jvm-clj.el --- Clojure access to Emacs Lisp values -*- lexical-binding: t; -*-

(require 'emacs-jni)

(defvar jvm-clj-classpath nil
  "List of paths (jar files or directories) added to the JVM classpath
for `jvm-clj-start', on top of `jvm-classpath'. Typically the user's
Clojure/nREPL jars (see test/fetch-clojure-jars.sh for known-working
versions). `jvm-clj-start' additionally adds this package's own
`clojure-src' directory, so it need not be listed here.")

(defvar jvm-clj--source-dir
  (expand-file-name "../clojure-src/"
                     (file-name-directory (or load-file-name buffer-file-name)))
  "Absolute path to this package's `clojure-src' directory.
Computed at load time (`load-file-name'/`buffer-file-name' are only
meaningful while this file itself is being loaded, unlike inside a
function called later -- see `emacs-jni--module-path' for the same
pattern).")

(defun jvm-clj-start (&optional timeout)
  "Start the JVM (if needed), load `jvm-clj-classpath' plus this
package's `clojure-src' directory, and require `dev.ublt.emacs.clj'.
Signals an error if the JVM fails to start, a classpath entry fails to
load, or the namespace fails to require (e.g. no Clojure jar on
`jvm-clj-classpath')."
  (unless (eq (jvm-status) 'ready)
    (unless (eq (jvm-start-sync nil nil timeout) 'ready)
      (error "jvm-clj-start: JVM failed to start (status: %s)" (jvm-status))))
  (dolist (path (append jvm-clj-classpath (list jvm-clj--source-dir)))
    (let ((result (jvm-load-jar-sync path timeout)))
      (unless (car result)
        (error "jvm-clj-start: failed to load %s: %s" path (cdr result)))))
  (let ((required (jvm-clojure-require "dev.ublt.emacs.clj" timeout)))
    (unless (car required)
      (error "jvm-clj-start: failed to require dev.ublt.emacs.clj: %s" (nth 2 required)))
    required))

(defun jvm-clj-nrepl-start (&optional port bind timeout)
  "Like `jvm-nrepl-start' (including its BIND argument), but ensures
`dev.ublt.emacs.clj' is loaded first."
  (jvm-clj-start timeout)
  (jvm-nrepl-start port bind timeout))

(defun jvm-clj-nrepl-stop (server &optional timeout)
  "Like `jvm-nrepl-stop'."
  (jvm-nrepl-stop server timeout))

(provide 'jvm-clj)
;;; jvm-clj.el ends here
