;;; jvm.el --- JVM embedding for Emacs -*- lexical-binding: t; -*-

;;; Commentary:

;; Start, pause, and stop an embedded JVM, load jars into it, and call Java
;; methods.  Each operation has three forms:
;;
;; - `-direct': runs on the Emacs main thread and blocks Emacs.
;; - `-async': sends a request to the JVM worker thread and returns at once.
;; - `-await': sends a request to the worker and waits for the reply.
;;
;; The unsuffixed names (`jvm-call', `jvm-start', ...) are aliases of the
;; canonical form.  See docs/explanation/architecture.md.

;;; Code:

(defvar jvm--source-directory (file-name-directory (macroexp-file-name))
  "Directory that holds this package's src/, java/, and clojure/ sources.
The default, the directory of this file, is the installed-package
layout.  A source checkout sets it in dev-paths.el.")

(defvar jvm-module-directory jvm--source-directory
  "Directory that holds the emacs-jni dynamic module.
The default, the directory of this file, is the installed-package
layout.  Set it before loading `jvm' to use another build, e.g. a
checkout's \"target/debug/\" (see dev-paths.el).")

(defun jvm--module-file ()
  "Return the path of the dynamic module in `jvm-module-directory'.
The file name is the one that cargo gives a cdylib on this platform."
  (expand-file-name (concat (if (eq system-type 'windows-nt) "" "lib")
                            "emacs_jni" module-file-suffix)
                    jvm-module-directory))

(unless (featurep 'emacs-jni)
  ;; `module-load' does not record the module's functions in
  ;; `load-history', so `symbol-file' returns nil for them, and
  ;; `find-function', xref, and `describe-function' cannot locate them.
  ;; Record them against the module file.  All of them are named jvm-*.
  (let ((file (jvm--module-file)))
    (module-load file)
    (push (cons file
                (mapcar (lambda (name) (cons 'defun (intern name)))
                        (all-completions
                         "jvm" obarray
                         (lambda (symbol)
                           (module-function-p (symbol-function symbol))))))
          load-history)))

(defun jvm--rust-defun-file (regexp)
  "Return the first .rs file under `jvm--source-directory' that matches REGEXP.
Search temporary copies, so that only the caller visits the file."
  (let ((src (expand-file-name "src" jvm--source-directory)))
    (when (file-directory-p src)
      (catch 'found
        (dolist (file (directory-files-recursively src (rx ".rs" eos)))
          (with-temp-buffer
            (insert-file-contents file)
            (when (re-search-forward regexp nil t)
              (throw 'found file))))))))

(defun jvm--find-rust-defun (symbol)
  "Visit the Rust `#[defun]' that defines SYMBOL in `jvm--source-directory'.
Return (BUFFER . POSITION), where POSITION is the start of the
docstring above the `#[defun]' (or of the `#[defun]' line if there is
no docstring).  Return nil if no Rust file defines SYMBOL."
  (let* ((regexp (rx "#[defun(" (* (not ")"))
                     "name = \"" (literal (symbol-name symbol)) "\""))
         (file (jvm--rust-defun-file regexp)))
    (when file
      (with-current-buffer (find-file-noselect file)
        (save-restriction
          (widen)
          (goto-char (point-min))
          (when (re-search-forward regexp nil t)
            (forward-line 0)
            (while (and (not (bobp))
                        (save-excursion
                          (forward-line -1)
                          (looking-at-p (rx (* blank) "///"))))
              (forward-line -1))
            (cons (current-buffer) (point))))))))

(defun jvm--find-function-search-for-symbol (find-fn symbol type library)
  "Around advice for `find-function-search-for-symbol'.
If LIBRARY is the emacs-jni module and TYPE is nil, return the Rust
`#[defun]' of SYMBOL, so that `find-function', xref, and the
`describe-function' link do not visit the binary module file.
Otherwise call FIND-FN with SYMBOL, TYPE, and LIBRARY."
  (or (and (null type)
           (stringp library)
           (file-equal-p library (jvm--module-file))
           (jvm--find-rust-defun symbol))
      (funcall find-fn symbol type library)))

(advice-add 'find-function-search-for-symbol :around
            #'jvm--find-function-search-for-symbol)

;; Primitives defined by the native module.
(declare-function jvm--init-channel "ext:emacs-jni" (process))
(declare-function jvm--start "ext:emacs-jni" (classpath options))
(declare-function jvm--pause "ext:emacs-jni" ())
(declare-function jvm--stop "ext:emacs-jni" ())
(declare-function jvm--load-jar "ext:emacs-jni" (path))
(declare-function jvm--load-jar-direct "ext:emacs-jni" (path))
(declare-function jvm--call "ext:emacs-jni" (target member args))
(declare-function jvm--call-direct "ext:emacs-jni" (target member args))
(declare-function jvm--service-inbound "ext:emacs-jni" (process data))
(declare-function jvm--invoke-reply "ext:emacs-jni" (id ok value))

(defvar jvm-classpath nil
  "List of paths added to the JVM's classpath when it starts.
Ignored when `jvm-start' resumes from `paused': the JVM already exists.")

(defvar jvm-options '("-Xrs")
  "List of JVM flag strings (e.g. \"-Xmx512m\") passed at `jvm-start' time.
Ignored when `jvm-start' resumes from `paused', as for `jvm-classpath'.

Keep \"-Xrs\" unless you need the JVM's signal handlers (e.g. for a
SIGQUIT thread dump).  Without it, the JVM handles SIGINT, SIGTERM,
SIGHUP, and SIGQUIT, and \\`C-g' can kill Emacs.  See
docs/explanation/architecture.md.")

(defvar jvm--status 'not-started
  "Current JVM lifecycle status.  See `jvm-status'.")

(defvar jvm-status-hook nil
  "Hook run with the new status symbol on every JVM status transition.")

(defvar jvm--pipe-process nil
  "The pipe process used to receive messages from the JVM worker thread.")

(defvar jvm--pending-load-jar nil
  "Alist of request-id -> callback for in-flight `jvm-load-jar-async' calls.")

(defvar jvm--pending-call nil
  "Alist of request-id -> callback for in-flight `jvm-call-async' calls.
`jvm-new-async' calls are included.")

(defvar jvm--lisp-callbacks (make-hash-table :test 'eql)
  "Handle -> Lisp function, for functions passed to Java as `LispFunction'.
`jvm--drop-callback' removes an entry when the Java proxy is collected.")

(defvar jvm--next-callback-handle 1
  "Monotonic counter for `jvm--lisp-callbacks' keys.")

(defvar jvm--log-buffer-name "*jvm-log*"
  "Name of the buffer that accumulates JVM log and status-failure messages.")

(defun jvm--log (format-string &rest args)
  "Append a line to the `*jvm-log*' buffer.
FORMAT-STRING and ARGS are as for `format'."
  (with-current-buffer (get-buffer-create jvm--log-buffer-name)
    (goto-char (point-max))
    (insert (apply #'format format-string args) "\n")))

;; Called by `jvm--service-inbound' with values already converted to Lisp.
;; The pipe carries only a wakeup byte.

(defun jvm--resolve-call (id ok value err)
  "Resolve the pending `jvm-call-async' callback for ID.
Call the callback with OK, VALUE, and ERR."
  (when-let ((callback (alist-get id jvm--pending-call)))
    (setq jvm--pending-call (assq-delete-all id jvm--pending-call))
    (funcall callback ok value err)))

(defun jvm--resolve-invoke (id handle args)
  "Call the callback registered for HANDLE with ARGS and reply to the JVM.
ID identifies the JVM request.  On a signal, reply with the
\(SYMBOL . DATA) that `condition-case' caught; the JVM caller gets it as
a `LispSignal'.  The handler catches t, not `error', so that `quit' also
completes the JVM future."
  (let ((fn (gethash handle jvm--lisp-callbacks)))
    (if (null fn)
        (jvm--invoke-reply
         id nil
         (cons 'error (list (format "no such callback handle %s" handle))))
      (condition-case err
          (jvm--invoke-reply id t (apply fn args))
        (t (jvm--invoke-reply id nil err))))))

(defun jvm--resolve-load-jar (id ok err)
  "Resolve the pending `jvm-load-jar-async' callback for ID.
Call the callback with OK and ERR."
  (when-let ((callback (alist-get id jvm--pending-load-jar)))
    (setq jvm--pending-load-jar (assq-delete-all id jvm--pending-load-jar))
    (funcall callback ok err)))

(defun jvm--apply-status (state detail)
  "Apply a JVM lifecycle transition to STATE, logging optional DETAIL."
  (setq jvm--status (intern state))
  (when detail (jvm--log "[status] %s: %s" state detail))
  (run-hook-with-args 'jvm-status-hook (intern state)))

(defun jvm--log-message (level text)
  "Append a [LEVEL] TEXT line to the `*jvm-log*' buffer and echo it."
  (jvm--log "[%s] %s" level text)
  (message "emacs-jni [%s]: %s" level text))

(defun jvm--drop-callback (handle)
  "Drop HANDLE's entry from `jvm--lisp-callbacks'.
Call this when the JVM proxy of HANDLE is garbage collected."
  (remhash handle jvm--lisp-callbacks))

(defun jvm--ensure-pipe-process ()
  "Create the pipe process and open the Rust-side channel, once."
  (unless (process-live-p jvm--pipe-process)
    (setq jvm--pipe-process
          (make-pipe-process
           :name "emacs-jni-channel"
           :filter #'jvm--service-inbound
           :noquery t))
    ;; Let any Lisp thread wait on this process with `accept-process-output'.
    (set-process-thread jvm--pipe-process nil)
    (jvm--init-channel jvm--pipe-process)))

(defun jvm-status ()
  "Return the current JVM lifecycle status symbol."
  jvm--status)

(defun jvm-start-async (&optional classpath options)
  "Start (or resume) the JVM asynchronously.
CLASSPATH and OPTIONS default to `jvm-classpath' and `jvm-options'.
If the `jvm--start' primitive signals an error (e.g. because the current
status does not allow starting), `jvm--status' reverts to its value
before this call, so it does not stay at `starting'."
  (jvm--ensure-pipe-process)
  (let ((previous-status jvm--status))
    (setq jvm--status 'starting)
    (run-hook-with-args 'jvm-status-hook 'starting)
    (condition-case err
        (jvm--start (or classpath jvm-classpath) (or options jvm-options))
      (error
       (setq jvm--status previous-status)
       (run-hook-with-args 'jvm-status-hook previous-status)
       (signal (car err) (cdr err))))))

(define-error 'jvm-timeout "Timed out waiting for the JVM")

(defun jvm-start-await (&optional classpath options timeout)
  "Like `jvm-start-async', but block until the JVM is ready.
CLASSPATH and OPTIONS are as for `jvm-start-async'.  Signal
`jvm-timeout' after TIMEOUT seconds (default 30)."
  (jvm-start-async classpath options)
  (let ((deadline (+ (float-time) (or timeout 30))))
    (while (eq (jvm-status) 'starting)
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (signal 'jvm-timeout (list 'jvm-start-await))))))
  (jvm-status))

(defalias 'jvm-start 'jvm-start-await
  "Canonical `jvm-start' is the synchronous variant.
See `jvm-start-async' for the non-blocking form.")

(defun jvm-pause-async ()
  "Pause the JVM asynchronously.
The worker thread detaches; the JVM state is kept.
If the `jvm--pause' primitive signals an error (e.g. because the current
status does not allow pausing), `jvm--status' reverts to its value
before this call, so it does not stay at `pausing'."
  (let ((previous-status jvm--status))
    (setq jvm--status 'pausing)
    (run-hook-with-args 'jvm-status-hook 'pausing)
    (condition-case err
        (jvm--pause)
      (error
       (setq jvm--status previous-status)
       (run-hook-with-args 'jvm-status-hook previous-status)
       (signal (car err) (cdr err))))))

(defun jvm-pause-await (&optional timeout)
  "Like `jvm-pause-async', but block until the JVM is paused.
Signal `jvm-timeout' after TIMEOUT seconds (default 30)."
  (jvm-pause-async)
  (let ((deadline (+ (float-time) (or timeout 30))))
    (while (eq (jvm-status) 'pausing)
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (signal 'jvm-timeout (list 'jvm-pause-await))))))
  (jvm-status))

(defalias 'jvm-pause 'jvm-pause-await
  "Canonical `jvm-pause' is the synchronous variant.
See `jvm-pause-async' for the non-blocking form.")

(defun jvm-stop-async ()
  "Try to fully tear down the JVM asynchronously.  Experimental.

Restarting the JVM in the same process after a stop is NOT supported:
the JVM does not support re-initialization in a process that already
destroyed one.  `jvm--stop' calls JNI_DestroyJavaVM, after which a later
`jvm-start' typically yields a non-functional \"zombie\" VM whose calls
fail with \"JNI call failed\".  A fresh Emacs process is the only
reliable reset.  To release JVM resources temporarily and resume later,
use `jvm-pause', not `jvm-stop'.

If the `jvm--stop' primitive signals an error (e.g. because the current
status does not allow stopping), `jvm--status' reverts to its value
before this call, so it does not stay at `stopping'."
  (let ((previous-status jvm--status))
    (setq jvm--status 'stopping)
    (run-hook-with-args 'jvm-status-hook 'stopping)
    (condition-case err
        (jvm--stop)
      (error
       (setq jvm--status previous-status)
       (run-hook-with-args 'jvm-status-hook previous-status)
       (signal (car err) (cdr err))))))

(defun jvm-stop-await (&optional timeout)
  "Like `jvm-stop-async', but block until `stopped' or `stop-failed'.
Signal `jvm-timeout' after TIMEOUT seconds (default 30)."
  (jvm-stop-async)
  (let ((deadline (+ (float-time) (or timeout 30))))
    (while (eq (jvm-status) 'stopping)
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (signal 'jvm-timeout (list 'jvm-stop-await))))))
  (jvm-status))

(defalias 'jvm-stop 'jvm-stop-await
  "Canonical `jvm-stop' is the synchronous variant.
See `jvm-stop-async' for the non-blocking form.")

(defun jvm-load-jar-async (path &optional callback)
  "Load the jar at PATH into the shared classloader asynchronously.
Return a request id immediately.  If CALLBACK is non-nil, call it as
\(CALLBACK OK ERR) once loading finishes."
  (let ((id (jvm--load-jar path)))
    (when callback
      (push (cons id callback) jvm--pending-load-jar))
    id))

(defun jvm-load-jar-await (path &optional timeout)
  "Like `jvm-load-jar-async' for PATH, but block until it finishes.
Return nil.  Signal `jvm-error' on failure, or `jvm-timeout' after
TIMEOUT seconds (default 30)."
  (let* ((result :pending)
         (id (jvm-load-jar-async
              path (lambda (ok err) (setq result (cons ok err)))))
         (deadline (+ (float-time) (or timeout 30))))
    (while (eq result :pending)
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (setq jvm--pending-load-jar
                (assq-delete-all id jvm--pending-load-jar))
          (signal 'jvm-timeout (list 'jvm-load-jar-await path)))))
    (unless (car result)
      (signal 'jvm-error (list (cdr result))))))

(defun jvm-load-jar-direct (path)
  "Load the jar or classpath directory at PATH synchronously.
Load it into the shared classloader, on this thread.  Return nil.
Signal `jvm-error' on failure.

Like `jvm-call-direct', this runs on the Emacs main thread with no
worker round trip.  It shares one classloader with
`jvm-load-jar-async', so a jar loaded either way is visible to both the
async and the direct call paths."
  (jvm--load-jar-direct path))

(defalias 'jvm-load-jar 'jvm-load-jar-direct
  "Canonical `jvm-load-jar' is the direct (main-thread) variant.
See `jvm-load-jar-async' for the non-blocking form and
`jvm-load-jar-await' for the worker-blocking form.")

(defun jvm-call-async (target member args &optional callback)
  "Call TARGET.MEMBER(ARGS...) on the JVM asynchronously.
TARGET is a class-name string (static call) or a wrapped JVM object
\(instance call).  ARGS is a list.  Return a request id immediately.  If
CALLBACK is non-nil, call it as (CALLBACK OK RESULT ERR) once the call
finishes."
  (let ((id (jvm--call target member args)))
    (when callback
      (push (cons id callback) jvm--pending-call))
    id))

(defun jvm-call-await (target member args &optional timeout)
  "Like `jvm-call-async', but block until the call finishes.
TARGET, MEMBER, and ARGS are as for `jvm-call-async'.  Return the result
value directly.  Signal `jvm-error' on a Java/Clojure failure, or
`jvm-timeout' after TIMEOUT seconds (default 30)."
  (let* ((result :pending)
         (id (jvm-call-async
              target member args
              (lambda (ok res err) (setq result (list ok res err)))))
         (deadline (+ (float-time) (or timeout 30))))
    (while (eq result :pending)
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (setq jvm--pending-call (assq-delete-all id jvm--pending-call))
          (signal 'jvm-timeout (list 'jvm-call-await target member)))))
    (if (car result)
        (nth 1 result)
      (signal 'jvm-error (list (nth 2 result))))))

(defun jvm-new-async (class-name args &optional callback)
  "Construct a new CLASS-NAME(ARGS...) instance on the JVM asynchronously.
Sugar over `jvm-call-async' with MEMBER \"<init>\".  CALLBACK is as for
`jvm-call-async'."
  (jvm-call-async class-name "<init>" args callback))

(defun jvm-new-await (class-name args &optional timeout)
  "Like `jvm-new-async' for CLASS-NAME and ARGS, but block.
TIMEOUT is as for `jvm-call-await'."
  (jvm-call-await class-name "<init>" args timeout))

(defun jvm-call-direct (target member args)
  "Call TARGET.MEMBER(ARGS...) on the JVM synchronously, on this thread.
Return the result value directly.  Signal `jvm-error' on failure.

Unlike `jvm-call-await' (async `jvm-call-async' plus a blocking wait on
the worker), this runs the call on the Emacs main thread itself, with
no worker round trip.  It blocks the Emacs main loop for the whole
call, so keep heavy or blocking work on the Clojure side's own threads."
  (jvm--call-direct target member args))

(defun jvm-new-direct (class-name args)
  "Construct CLASS-NAME(ARGS...) synchronously on this thread.
Sugar over `jvm-call-direct' with MEMBER \"<init>\"."
  (jvm-call-direct class-name "<init>" args))

(defalias 'jvm-call 'jvm-call-direct
  "Canonical `jvm-call' is the direct (main-thread) variant.
See `jvm-call-async' for the non-blocking form and `jvm-call-await' for
the worker-blocking form.")

(defalias 'jvm-new 'jvm-new-direct
  "Canonical `jvm-new' is the direct (main-thread) variant.
See `jvm-new-async' for the non-blocking form and `jvm-new-await' for
the worker-blocking form.")

(defun jvm--register-callback (fn)
  "Register FN in `jvm--lisp-callbacks' and return its fresh handle."
  (let ((handle jvm--next-callback-handle))
    (setq jvm--next-callback-handle (1+ jvm--next-callback-handle))
    (puthash handle fn jvm--lisp-callbacks)
    handle))

(provide 'jvm)
;;; jvm.el ends here
