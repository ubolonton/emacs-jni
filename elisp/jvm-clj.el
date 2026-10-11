;;; jvm-clj.el --- Clojure on the embedded JVM -*- lexical-binding: t; -*-

;;; Commentary:

;; Start Clojure in the JVM from `jvm', evaluate Clojure code, require
;; namespaces, and run an nREPL server.  As in `jvm', each operation has a
;; `-direct' form (Emacs main thread) and an `-await' form (JVM worker
;; thread).  The unsuffixed names are aliases of the `-direct' form.

;;; Code:

(require 'jvm)

(defvar jvm-clj-classpath nil
  "Jar files or directories added to the JVM classpath by `jvm-clj-start'.
If nil, `jvm-clj-start' uses `jvm-clj-ensure-jars', which downloads
`jvm-clj-dependencies'.  `jvm-clj-start' also adds this package's
`clojure'.")

(defvar jvm-clj-dependencies
  '((:group "org.clojure" :artifact "clojure" :version "1.12.0"
     :repository "https://repo1.maven.org/maven2"
     :sha256 "c45333006441a059ea9fdb1341fc6c1f40b921a10dccd82665311e48a0384763")
    (:group "org.clojure" :artifact "spec.alpha" :version "0.5.238"
     :repository "https://repo1.maven.org/maven2"
     :sha256 "94cd99b6ea639641f37af4860a643b6ed399ee5a8be5d717cff0b663c8d75077")
    (:group "org.clojure" :artifact "core.specs.alpha" :version "0.4.74"
     :repository "https://repo1.maven.org/maven2"
     :sha256 "eb73ac08cf49ba840c88ba67beef11336ca554333d9408808d78946e0feb9ddb")
    ;; nREPL is published to Clojars, not Maven Central.
    (:group "nrepl" :artifact "nrepl" :version "1.7.0"
     :repository "https://repo.clojars.org"
     :sha256 "d07390277190e0a974e1497fd4485005a4964f9f556a50bd530509f48ba65c1b"))
  "Jars that `jvm-clj-ensure-jars' downloads: Clojure, its deps, and nREPL.
Each entry is a plist with :group, :artifact, :version, :repository (a
Maven repository URL), and :sha256 (of the jar).  To use other versions,
change the entries; a jar whose sha256 differs is rejected.")

(defvar jvm-clj-maven-local-repo "~/.m2/repository"
  "Local Maven repository where `jvm-clj-ensure-jars' caches jars.
The default is the one that Maven and Clojure CLI (`clj') use, so jars
are shared with them.")

(defvar jvm-clj--source-dir
  (expand-file-name "clojure/"
                    (file-name-directory
                     (or load-file-name buffer-file-name)))
  "Absolute path to this package's `clojure' directory.
A source checkout sets it in dev-paths.el.")

;;; Clojure interop primitives (worker await path).

(defun jvm-clj--var-await (ns name)
  "Resolve the wrapped IFn for NS/NAME via `clojure.java.api.Clojure/var'.
Return the IFn value.  Signal `jvm-error' on failure."
  (jvm-call-await "clojure.java.api.Clojure" "var" (list ns name)))

(defun jvm-clj--read-await (form-string)
  "Parse FORM-STRING as Clojure data via `clojure.java.api.Clojure/read'.
Return the data value.  Signal `jvm-error' on failure."
  (jvm-call-await "clojure.java.api.Clojure" "read" (list form-string)))

(defun jvm-clj--invoke-with-form-await (var-ns var-name form-string
                                               &optional timeout)
  "Invoke the IFn VAR-NS/VAR-NAME with FORM-STRING read as Clojure data.
Return the result.  Signal `jvm-error' on failure, or `jvm-timeout'
after TIMEOUT seconds (default 30)."
  (let ((fn (jvm-clj--var-await var-ns var-name))
        (form (jvm-clj--read-await form-string)))
    (jvm-call-await fn "invoke" (list form) timeout)))

(defun jvm-clj-require-await (namespace &optional timeout)
  "Require the Clojure NAMESPACE (a string, e.g. \"clojure.set\").
Return the result value.  Signal `jvm-error' on failure, or
`jvm-timeout' after TIMEOUT seconds (default 30)."
  (jvm-clj--invoke-with-form-await "clojure.core" "require" namespace
                                   timeout))

(defun jvm-clj-eval-string-await (code &optional timeout)
  "Evaluate CODE (a string of Clojure source) and return its result value.
Signal `jvm-error' on failure, or `jvm-timeout' after TIMEOUT seconds
\(default 30).

`*ns*' set by `in-ns' does not persist to the next call, because the
eval binds `*ns*' for that call only.  Put the namespace switch in CODE.

CODE can contain many top-level forms; they are evaluated in order, and
the value of the last one is returned.  Keywords and symbols return as
Lisp keywords and symbols; vectors and maps return as opaque JVM
objects."
  (jvm-call-await (jvm-clj--var-await "emacs-jni.clj" "eval-for-lisp")
                  "invoke" (list code) timeout))

;;; Clojure interop primitives (direct main-thread path).
;;;
;;; Each step goes through `jvm-call-direct': the call runs on the Emacs
;;; main thread, which stays attached to the JVM across calls, so
;;; successive direct calls share one JVM thread (stable identity,
;;; persistent Java `ThreadLocal' values) and a Clojure->Lisp callback can
;;; reenter Lisp without deadlock.  A failure signals `jvm-error'; a Lisp
;;; signal/throw raised in a reentrant callback propagates out as itself.

(defun jvm-clj--var-direct (ns name)
  "Resolve the wrapped IFn for NS/NAME on this thread.
Direct counterpart of `jvm-clj--var-await'.  Return the IFn value.
Signal `jvm-error' on failure."
  (jvm-call-direct "clojure.java.api.Clojure" "var" (list ns name)))

(defun jvm-clj--read-direct (form-string)
  "Parse FORM-STRING as Clojure data on this thread.
Direct counterpart of `jvm-clj--read-await'.  Return the data value.
Signal `jvm-error' on failure."
  (jvm-call-direct "clojure.java.api.Clojure" "read" (list form-string)))

(defun jvm-clj--invoke-with-form-direct (var-ns var-name form-string)
  "Invoke the IFn VAR-NS/VAR-NAME with FORM-STRING read as Clojure data.
Direct counterpart of `jvm-clj--invoke-with-form-await'."
  (let* ((fn (jvm-clj--var-direct var-ns var-name))
         (form (jvm-clj--read-direct form-string)))
    (jvm-call-direct fn "invoke" (list form))))

(defun jvm-clj-require-direct (namespace)
  "Require the Clojure NAMESPACE on this thread.
Direct counterpart of `jvm-clj-require-await'."
  (jvm-clj--invoke-with-form-direct "clojure.core" "require" namespace))

(defalias 'jvm-clj-require 'jvm-clj-require-direct
  "Canonical `jvm-clj-require' is the direct (main-thread) variant.
See `jvm-clj-require-await' for the worker-blocking form.")

(defun jvm-clj-eval-string-direct (code)
  "Evaluate CODE on the Emacs main thread and return its result.
Direct counterpart of `jvm-clj-eval-string-await'.

The main thread stays attached to the JVM, so Java `ThreadLocal' values
persist across calls.  `*ns*' does not persist; see
`jvm-clj-eval-string-await'.  Emacs is blocked for the whole eval; run
slow work on Clojure's own threads."
  (jvm-call-direct (jvm-clj--var-direct "emacs-jni.clj" "eval-for-lisp")
                   "invoke" (list code)))

(defalias 'jvm-clj-eval-string 'jvm-clj-eval-string-direct
  "Canonical `jvm-clj-eval-string' is the direct (main-thread) variant.
See `jvm-clj-eval-string-await' for the worker-blocking form.")

;;; Dependencies.

(defun jvm-clj--jar-relative-path (dependency)
  "Return the Maven repository path of DEPENDENCY's jar."
  (let ((group (plist-get dependency :group))
        (artifact (plist-get dependency :artifact))
        (version (plist-get dependency :version)))
    (format "%s/%s/%s/%s-%s.jar"
            (subst-char-in-string ?. ?/ group) artifact version artifact version)))

(defun jvm-clj--check-sha256 (file expected)
  "Signal an error unless FILE's sha256 is EXPECTED."
  (let ((actual (with-temp-buffer
                  (set-buffer-multibyte nil)
                  (insert-file-contents-literally file)
                  (secure-hash 'sha256 (current-buffer)))))
    (unless (equal actual expected)
      (error "jvm-clj: %s has sha256 %s, expected %s" file actual expected))))

(defun jvm-clj--ensure-jar (dependency)
  "Return the local path of DEPENDENCY's jar.  Download it if missing.
Check the sha256 of a cached jar and of a downloaded one.  A download
goes to a temporary file first, so a failed or corrupt download does
not stay in the cache."
  (let* ((relative (jvm-clj--jar-relative-path dependency))
         (local (expand-file-name relative jvm-clj-maven-local-repo))
         (expected (plist-get dependency :sha256)))
    (if (file-exists-p local)
        (jvm-clj--check-sha256 local expected)
      (make-directory (file-name-directory local) t)
      (let ((url (concat (plist-get dependency :repository) "/" relative))
            (temp (make-temp-file
                   (expand-file-name ".download-" (file-name-directory local)))))
        (unwind-protect
            (progn
              (message "jvm-clj: downloading %s" url)
              (url-copy-file url temp t)
              (jvm-clj--check-sha256 temp expected)
              (rename-file temp local))
          (when (file-exists-p temp)
            (delete-file temp)))))
    local))

(defun jvm-clj-ensure-jars ()
  "Return the local paths of the jars in `jvm-clj-dependencies'.
Download a missing jar into `jvm-clj-maven-local-repo'.  This blocks
Emacs.  Signal an error if a jar's sha256 does not match."
  (mapcar #'jvm-clj--ensure-jar jvm-clj-dependencies))

(defun jvm-clj--classpath ()
  "Return the classpath entries that `jvm-clj-start' loads."
  (append (or jvm-clj-classpath (jvm-clj-ensure-jars))
          (list jvm-clj--source-dir)))

;;; Startup.

(defun jvm-clj-start-await (&optional timeout)
  "Start the JVM if needed, then load and require `emacs-jni.clj'.
Signal an error if the JVM does not start, a classpath entry does not
load, or the namespace cannot be required (e.g. no Clojure jar on
`jvm-clj-classpath').  Each step waits up to TIMEOUT seconds
\(default 30)."
  (unless (eq (jvm-status) 'ready)
    (unless (eq (jvm-start-await nil nil timeout) 'ready)
      (error "jvm-clj-start-await: JVM failed to start (status: %s)"
             (jvm-status))))
  (dolist (path (jvm-clj--classpath))
    (jvm-load-jar-await path timeout))
  (jvm-clj-require-await "emacs-jni.clj" timeout))

(defun jvm-clj-start-direct (&optional timeout)
  "Like `jvm-clj-start-await', but require `emacs-jni.clj' on this thread.
JVM start and jar loading still use the worker, and wait up to TIMEOUT
seconds (default 30) each."
  (unless (eq (jvm-status) 'ready)
    (unless (eq (jvm-start-await nil nil timeout) 'ready)
      (error "jvm-clj-start-direct: JVM failed to start (status: %s)"
             (jvm-status))))
  (dolist (path (jvm-clj--classpath))
    (jvm-load-jar-await path timeout))
  (jvm-clj-require-direct "emacs-jni.clj"))

(defalias 'jvm-clj-start 'jvm-clj-start-direct
  "Canonical `jvm-clj-start' is the direct (main-thread) variant.
See `jvm-clj-start-await' for the worker-blocking form.")

;;; nREPL server.  One layer: always ensures `emacs-jni.clj' is loaded first.

(defun jvm-clj-nrepl-start-await (&optional port bind timeout)
  "Start an nREPL server, after `jvm-clj-start-await'.
PORT defaults to 0 (the OS picks a port).  BIND is the listen address;
the default is loopback.  Bind to another address only if you must,
e.g. for VM port forwarding; see
docs/how-to-guides/run-an-nrepl-server.md for the security cost.  Each
step waits up to TIMEOUT seconds (default 30).

Return (ACTUAL-PORT . SERVER).  Pass SERVER to
`jvm-clj-nrepl-stop-await'."
  (jvm-clj-start-await timeout)
  (jvm-clj-require-await "nrepl.server" timeout)
  (let* ((start-fn (jvm-clj--var-await "nrepl.server" "start-server"))
         (port-kw (jvm-clj--read-await ":port"))
         (start-args (list port-kw (or port 0))))
    ;; Add `:bind' only when requested.
    (when bind
      (setq start-args
            (append start-args (list (jvm-clj--read-await ":bind") bind))))
    (let* ((server (jvm-call-await start-fn "invoke" start-args timeout))
           (bound-port (jvm-call-await port-kw "invoke" (list server)
                                       timeout)))
      (cons bound-port server))))

(defun jvm-clj-nrepl-stop-await (server &optional timeout)
  "Stop SERVER (from `jvm-clj-nrepl-start-await').
Return the result value.  Signal `jvm-error' on failure, or
`jvm-timeout' after TIMEOUT seconds (default 30)."
  (jvm-call-await (jvm-clj--var-await "nrepl.server" "stop-server")
                  "invoke" (list server) timeout))

(defun jvm-clj-nrepl-start-direct (&optional port bind)
  "Direct (main-thread) counterpart of `jvm-clj-nrepl-start-await'.
PORT and BIND are as in `jvm-clj-nrepl-start-await'."
  (jvm-clj-start-direct)
  (jvm-clj-require-direct "nrepl.server")
  (let* ((start-fn (jvm-clj--var-direct "nrepl.server" "start-server"))
         (port-kw (jvm-clj--read-direct ":port"))
         (start-args (list port-kw (or port 0))))
    (when bind
      (setq start-args
            (append start-args (list (jvm-clj--read-direct ":bind") bind))))
    (let* ((server (jvm-call-direct start-fn "invoke" start-args))
           (bound-port (jvm-call-direct port-kw "invoke" (list server))))
      (cons bound-port server))))

(defalias 'jvm-clj-nrepl-start 'jvm-clj-nrepl-start-direct
  "Canonical `jvm-clj-nrepl-start' is the direct (main-thread) variant.
See `jvm-clj-nrepl-start-await' for the worker-blocking form.")

(defun jvm-clj-nrepl-stop-direct (server)
  "Stop SERVER (from `jvm-clj-nrepl-start-direct') on this thread.
Direct counterpart of `jvm-clj-nrepl-stop-await'."
  (jvm-call-direct (jvm-clj--var-direct "nrepl.server" "stop-server")
                   "invoke" (list server)))

(defalias 'jvm-clj-nrepl-stop 'jvm-clj-nrepl-stop-direct
  "Canonical `jvm-clj-nrepl-stop' is the direct (main-thread) variant.
See `jvm-clj-nrepl-stop-await' for the worker-blocking form.")

(provide 'jvm-clj)
;;; jvm-clj.el ends here
