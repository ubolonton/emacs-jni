;;; jvm-clj.el --- Clojure access on top of the embedded JVM -*- lexical-binding: t; -*-

(require 'jvm)

(defvar jvm-clj-classpath nil
  "Jar files or directories added to the JVM classpath by `jvm-clj-start'.
Usually the Clojure and nREPL jars; see test/fetch-clojure-jars.sh for
working versions. `jvm-clj-start' also adds this package's `clojure-src'.")

(defvar jvm-clj--source-dir
  (expand-file-name "../clojure-src/"
                     (file-name-directory (or load-file-name buffer-file-name)))
  "Absolute path to this package's `clojure-src' directory.")

;;; Clojure interop primitives (worker await path).

(defun jvm-clj--var-await (ns name)
  "Resolve the wrapped IFn for NS/NAME via `clojure.java.api.Clojure/var'.
Returns the IFn value; signals `jvm-error' on failure."
  (jvm-call-await "clojure.java.api.Clojure" "var" (list ns name)))

(defun jvm-clj--read-await (form-string)
  "Parse FORM-STRING as Clojure data via `clojure.java.api.Clojure/read'.
Returns the data value; signals `jvm-error' on failure."
  (jvm-call-await "clojure.java.api.Clojure" "read" (list form-string)))

(defun jvm-clj--invoke-with-form-await (var-ns var-name form-string &optional timeout)
  "Invoke the IFn VAR-NS/VAR-NAME with FORM-STRING read as Clojure data.
Returns the result; signals `jvm-error' on failure."
  (let ((fn (jvm-clj--var-await var-ns var-name))
        (form (jvm-clj--read-await form-string)))
    (jvm-call-await fn "invoke" (list form) timeout)))

(defun jvm-clj-require-await (namespace &optional timeout)
  "Require the Clojure NAMESPACE (a string, e.g. \"clojure.set\").
Returns the result value; signals `jvm-error' on failure."
  (jvm-clj--invoke-with-form-await "clojure.core" "require" namespace timeout))

(defun jvm-clj-eval-string-await (code &optional timeout)
  "Evaluate CODE (a string of Clojure source) and return its result value.
Signals `jvm-error' on failure.

Each call can run on a different JVM thread, so `*ns*' set by `in-ns'
does not persist to the next call. Put the namespace switch in CODE, or
use `jvm-clj-eval-string-direct'.

CODE can contain many top-level forms; they are evaluated in order, and
the value of the last one is returned. Keywords and symbols return as Lisp
keywords and symbols; vectors and maps return as opaque JVM objects."
  (jvm-call-await (jvm-clj--var-await "emacs-jni.clj" "eval-for-lisp")
                  "invoke" (list code) timeout))

;;; Clojure interop primitives (direct main-thread path).
;;;
;;; Each step goes through `jvm-call-direct': the call runs on the Emacs
;;; main thread, which stays attached to the JVM across calls, so successive
;;; direct calls share one JVM thread (stable identity, persistent thread-
;;; locals) and a Clojure->Lisp callback can reenter Lisp without deadlock.
;;; A failure signals `jvm-error'; a Lisp signal/throw raised in a reentrant
;;; callback propagates out as itself.

(defun jvm-clj--var-direct (ns name)
  "Direct counterpart of `jvm-clj--var-await': resolve the wrapped IFn for NS/NAME.
Returns the IFn value directly; signals `jvm-error' on failure."
  (jvm-call-direct "clojure.java.api.Clojure" "var" (list ns name)))

(defun jvm-clj--read-direct (form-string)
  "Direct counterpart of `jvm-clj--read-await': parse FORM-STRING as Clojure data.
Returns the data value directly; signals `jvm-error' on failure."
  (jvm-call-direct "clojure.java.api.Clojure" "read" (list form-string)))

(defun jvm-clj--invoke-with-form-direct (var-ns var-name form-string)
  "Direct counterpart of `jvm-clj--invoke-with-form-await'."
  (let* ((fn (jvm-clj--var-direct var-ns var-name))
         (form (jvm-clj--read-direct form-string)))
    (jvm-call-direct fn "invoke" (list form))))

(defun jvm-clj-require-direct (namespace)
  "Direct counterpart of `jvm-clj-require-await'.
Requires the Clojure NAMESPACE on this thread."
  (jvm-clj--invoke-with-form-direct "clojure.core" "require" namespace))

(defalias 'jvm-clj-require 'jvm-clj-require-direct
  "Canonical `jvm-clj-require' is the direct (main-thread) variant. See
`jvm-clj-require-await' for the worker-blocking form.")

(defun jvm-clj-eval-string-direct (code)
  "Direct counterpart of `jvm-clj-eval-string-await'.
Evaluates CODE on the Emacs main thread and returns its result.

The main thread stays attached to the JVM, so `*ns*' and dynamic bindings
persist across calls. Emacs is blocked for the whole eval; run slow work
on Clojure's own threads."
  (jvm-call-direct (jvm-clj--var-direct "emacs-jni.clj" "eval-for-lisp")
                   "invoke" (list code)))

(defalias 'jvm-clj-eval-string 'jvm-clj-eval-string-direct
  "Canonical `jvm-clj-eval-string' is the direct (main-thread) variant. See
`jvm-clj-eval-string-await' for the worker-blocking form.")

;;; Startup.

(defun jvm-clj-start-await (&optional timeout)
  "Start the JVM if needed, then load and require `emacs-jni.clj'.
Signals an error if the JVM does not start, a classpath entry does not
load, or the namespace cannot be required (e.g. no Clojure jar on
`jvm-clj-classpath')."
  (unless (eq (jvm-status) 'ready)
    (unless (eq (jvm-start-await nil nil timeout) 'ready)
      (error "jvm-clj-start-await: JVM failed to start (status: %s)" (jvm-status))))
  (dolist (path (append jvm-clj-classpath (list jvm-clj--source-dir)))
    (jvm-load-jar-await path timeout))
  (jvm-clj-require-await "emacs-jni.clj" timeout))

(defun jvm-clj-start-direct (&optional timeout)
  "Like `jvm-clj-start-await', but require `emacs-jni.clj' on the main thread.
JVM start and jar loading still use the worker."
  (unless (eq (jvm-status) 'ready)
    (unless (eq (jvm-start-await nil nil timeout) 'ready)
      (error "jvm-clj-start-direct: JVM failed to start (status: %s)" (jvm-status))))
  (dolist (path (append jvm-clj-classpath (list jvm-clj--source-dir)))
    (jvm-load-jar-await path timeout))
  (jvm-clj-require-direct "emacs-jni.clj"))

(defalias 'jvm-clj-start 'jvm-clj-start-direct
  "Canonical `jvm-clj-start' is the direct (main-thread) variant. See
`jvm-clj-start-await' for the worker-blocking form.")

;;; nREPL server. One layer: always ensures `emacs-jni.clj' is loaded first.

(defun jvm-clj-nrepl-start-await (&optional port bind timeout)
  "Start an nREPL server, after `jvm-clj-start-await'.
PORT defaults to 0 (the OS picks a port). BIND is the listen address;
the default is loopback. Bind to another address only if you must, e.g.
for VM port forwarding; see docs/how-to-guides/run-an-nrepl-server.md
for the security cost.

Returns (ACTUAL-PORT . SERVER). Pass SERVER to `jvm-clj-nrepl-stop-await'."
  (jvm-clj-start-await timeout)
  (jvm-clj-require-await "nrepl.server" timeout)
  (let* ((start-fn (jvm-clj--var-await "nrepl.server" "start-server"))
         (port-kw (jvm-clj--read-await ":port"))
         (start-args (list port-kw (or port 0))))
    ;; Add `:bind' only when requested.
    (when bind
      (setq start-args (append start-args (list (jvm-clj--read-await ":bind") bind))))
    (let* ((server (jvm-call-await start-fn "invoke" start-args timeout))
           (bound-port (jvm-call-await port-kw "invoke" (list server) timeout)))
      (cons bound-port server))))

(defun jvm-clj-nrepl-stop-await (server &optional timeout)
  "Stop SERVER (from `jvm-clj-nrepl-start-await'). Returns the result value;
signals `jvm-error' on failure."
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
      (setq start-args (append start-args (list (jvm-clj--read-direct ":bind") bind))))
    (let* ((server (jvm-call-direct start-fn "invoke" start-args))
           (bound-port (jvm-call-direct port-kw "invoke" (list server))))
      (cons bound-port server))))

(defalias 'jvm-clj-nrepl-start 'jvm-clj-nrepl-start-direct
  "Canonical `jvm-clj-nrepl-start' is the direct (main-thread) variant. See
`jvm-clj-nrepl-start-await' for the worker-blocking form.")

(defun jvm-clj-nrepl-stop-direct (server)
  "Direct counterpart of `jvm-clj-nrepl-stop-await'."
  (jvm-call-direct (jvm-clj--var-direct "nrepl.server" "stop-server")
                   "invoke" (list server)))

(defalias 'jvm-clj-nrepl-stop 'jvm-clj-nrepl-stop-direct
  "Canonical `jvm-clj-nrepl-stop' is the direct (main-thread) variant. See
`jvm-clj-nrepl-stop-await' for the worker-blocking form.")

(provide 'jvm-clj)
;;; jvm-clj.el ends here
