;;; jvm.el --- JVM embedding for Emacs -*- lexical-binding: t; -*-

(defvar jvm--module-path
  (or (getenv "EMACS_JNI_MODULE_PATH")
      (expand-file-name "../target/debug/libemacs_jni.so"
                         (file-name-directory (or load-file-name buffer-file-name))))
  "Path to the built emacs-jni dynamic module (.so file).")

(unless (featurep 'emacs-jni)
  (module-load jvm--module-path))

(defvar jvm-classpath nil
  "List of paths added to the JVM's classpath when it starts.
Ignored when `jvm-start' resumes from `paused': the JVM already exists.")

(defvar jvm-options '("-Xrs")
  "List of JVM flag strings (e.g. \"-Xmx512m\") passed at `jvm-start' time.
Ignored when `jvm-start' resumes from `paused', as for `jvm-classpath'.

Keep \"-Xrs\" unless you need the JVM's signal handlers (e.g. for a
SIGQUIT thread dump). Without it, the JVM handles SIGINT, SIGTERM, SIGHUP,
and SIGQUIT, and `C-g' can kill Emacs. See docs/explanation/architecture.md.")

(defvar jvm--status 'not-started
  "Current JVM lifecycle status. See `jvm-status'.")

(defvar jvm-status-hook nil
  "Hook run with the new status symbol on every JVM status transition.")

(defvar jvm--pipe-process nil
  "The pipe process used to receive messages from the JVM worker thread.")

(defvar jvm--pending-load-jar nil
  "Alist of request-id -> callback for in-flight `jvm-load-jar-async' calls.")

(defvar jvm--pending-call nil
  "Alist of request-id -> callback for in-flight `jvm-call-async'/`jvm-new-async' calls.")

(defvar jvm--log-buffer-name "*jvm-log*"
  "Name of the buffer that accumulates JVM log and status-failure messages.")

(defun jvm--log (format-string &rest args)
  "Append a line to the `*jvm-log*' buffer, formatted like `format'."
  (with-current-buffer (get-buffer-create jvm--log-buffer-name)
    (goto-char (point-max))
    (insert (apply #'format format-string args) "\n")))

;; Called by `jvm--service-inbound' with values already converted to Lisp.
;; The pipe carries only a wakeup byte.

(defun jvm--resolve-call (id ok value err)
  "Resolve the pending `jvm-call-async' callback for ID."
  (when-let ((callback (alist-get id jvm--pending-call)))
    (setq jvm--pending-call (assq-delete-all id jvm--pending-call))
    (funcall callback ok value err)))

(defun jvm--resolve-invoke (id handle args)
  "Call the callback registered for HANDLE with ARGS and reply to the JVM.
On a signal, reply with the `(SYMBOL . DATA)' that `condition-case'
caught; the JVM caller gets it as a `LispSignal'. The handler catches `t',
not `error', so that `quit' also completes the JVM future."
  (let ((fn (gethash handle jvm--lisp-callbacks)))
    (if (null fn)
        (jvm--invoke-reply id nil (cons 'error (list (format "no such callback handle %s" handle))))
      (condition-case err
          (jvm--invoke-reply id t (apply fn args))
        (t (jvm--invoke-reply id nil err))))))

(defun jvm--resolve-load-jar (id ok err)
  "Resolve the pending `jvm-load-jar-async' callback for ID."
  (when-let ((callback (alist-get id jvm--pending-load-jar)))
    (setq jvm--pending-load-jar (assq-delete-all id jvm--pending-load-jar))
    (funcall callback ok err)))

(defun jvm--apply-status (state detail)
  "Apply a JVM lifecycle transition to STATE, logging optional DETAIL."
  (setq jvm--status (intern state))
  (when detail (jvm--log "[status] %s: %s" state detail))
  (run-hook-with-args 'jvm-status-hook (intern state)))

(defun jvm--log-message (level text)
  "Append a `[LEVEL] TEXT' line to the `*jvm-log*' buffer and echo it."
  (jvm--log "[%s] %s" level text)
  (message "emacs-jni [%s]: %s" level text))

(defun jvm--drop-callback (handle)
  "Drop HANDLE's entry from `jvm--lisp-callbacks' (its JVM proxy was GC'd)."
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
status doesn't allow starting), `jvm--status' is reverted to what it was
before this call, so it doesn't get stuck at `starting'."
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
  "Like `jvm-start-async', but blocks until the JVM is ready or TIMEOUT (default 30s) elapses."
  (jvm-start-async classpath options)
  (let ((deadline (+ (float-time) (or timeout 30))))
    (while (eq (jvm-status) 'starting)
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (signal 'jvm-timeout (list 'jvm-start-await))))))
  (jvm-status))

(defalias 'jvm-start 'jvm-start-await
  "Canonical `jvm-start' is the synchronous variant. See `jvm-start-async' for
the non-blocking form.")

(defun jvm-pause-async ()
  "Pause the JVM asynchronously (detaches the worker thread; JVM state is retained).
If the `jvm--pause' primitive signals an error (e.g. because the current
status doesn't allow pausing), `jvm--status' is reverted to what it was
before this call, so it doesn't get stuck at `pausing'."
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
  "Like `jvm-pause-async', but blocks until paused or TIMEOUT (default 30s) elapses."
  (jvm-pause-async)
  (let ((deadline (+ (float-time) (or timeout 30))))
    (while (eq (jvm-status) 'pausing)
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (signal 'jvm-timeout (list 'jvm-pause-await))))))
  (jvm-status))

(defalias 'jvm-pause 'jvm-pause-await
  "Canonical `jvm-pause' is the synchronous variant. See `jvm-pause-async' for
the non-blocking form.")

(defun jvm-stop-async ()
  "Attempt to fully tear down the JVM asynchronously. Experimental.

Restarting the JVM in the same process after a stop is NOT supported: the
JVM does not support re-initialization in a process that already destroyed
one. `jvm--stop' calls JNI_DestroyJavaVM, after which a later `jvm-start'
typically yields a non-functional \"zombie\" VM whose calls fail with
\"JNI call failed\". A fresh Emacs process is the only reliable reset. To
release JVM resources temporarily and resume later, use `jvm-pause', not
`jvm-stop'.

If the `jvm--stop' primitive signals an error (e.g. because the current
status doesn't allow stopping), `jvm--status' is reverted to what it was
before this call, so it doesn't get stuck at `stopping'."
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
  "Like `jvm-stop-async', but blocks until stopped/stop-failed or TIMEOUT (default 30s)."
  (jvm-stop-async)
  (let ((deadline (+ (float-time) (or timeout 30))))
    (while (eq (jvm-status) 'stopping)
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (signal 'jvm-timeout (list 'jvm-stop-await))))))
  (jvm-status))

(defalias 'jvm-stop 'jvm-stop-await
  "Canonical `jvm-stop' is the synchronous variant. See `jvm-stop-async' for
the non-blocking form.")

(defun jvm-load-jar-async (path &optional callback)
  "Load the jar at PATH into the shared classloader asynchronously.
Returns a request id immediately. If CALLBACK is given, it is called as
\(CALLBACK OK ERR) once loading finishes."
  (let ((id (jvm--load-jar path)))
    (when callback
      (push (cons id callback) jvm--pending-load-jar))
    id))

(defun jvm-load-jar-await (path &optional timeout)
  "Like `jvm-load-jar-async', but blocks until it finishes (or TIMEOUT,
default 30s). Returns nil; signals `jvm-error' on failure, or `jvm-timeout'
if TIMEOUT elapses first."
  (let* ((result :pending)
         (id (jvm-load-jar-async path (lambda (ok err) (setq result (cons ok err)))))
         (deadline (+ (float-time) (or timeout 30))))
    (while (eq result :pending)
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (setq jvm--pending-load-jar (assq-delete-all id jvm--pending-load-jar))
          (signal 'jvm-timeout (list 'jvm-load-jar-await path)))))
    (unless (car result)
      (signal 'jvm-error (list (cdr result))))))

(defun jvm-load-jar-direct (path)
  "Load the jar or classpath directory at PATH into the shared classloader
synchronously, on this thread. Returns nil; signals `jvm-error' on failure.

Like `jvm-call-direct', this runs on the Emacs main thread with no worker
round trip. It shares one classloader with `jvm-load-jar-async', so a jar
loaded either way is visible to both the async and the direct call paths."
  (jvm--load-jar-direct path))

(defalias 'jvm-load-jar 'jvm-load-jar-direct
  "Canonical `jvm-load-jar' is the direct (main-thread) variant. See
`jvm-load-jar-async' for the non-blocking form and `jvm-load-jar-await' for
the worker-blocking form.")

(defun jvm-call-async (target member args &optional callback)
  "Call TARGET.MEMBER(ARGS...) on the JVM asynchronously.
TARGET is a class-name string (static call) or a wrapped JVM object
\(instance call). ARGS is a list. Returns a request id immediately. If
CALLBACK is given, it is called as (CALLBACK OK RESULT ERR) once the call
finishes."
  (let ((id (jvm--call target member args)))
    (when callback
      (push (cons id callback) jvm--pending-call))
    id))

(defun jvm-call-await (target member args &optional timeout)
  "Like `jvm-call-async', but blocks until the call finishes (or TIMEOUT,
default 30s). Returns the result value directly; signals `jvm-error' on a
Java/Clojure failure, or `jvm-timeout' if TIMEOUT elapses first."
  (let* ((result :pending)
         (id (jvm-call-async target member args
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
Sugar over `jvm-call-async' with MEMBER `\"<init>\"'."
  (jvm-call-async class-name "<init>" args callback))

(defun jvm-new-await (class-name args &optional timeout)
  "Like `jvm-new-async', but blocks. See `jvm-call-await'."
  (jvm-call-await class-name "<init>" args timeout))

(defun jvm-call-direct (target member args)
  "Call TARGET.MEMBER(ARGS...) on the JVM synchronously, on this thread.
Returns the result value directly; signals `jvm-error' on failure.

Unlike `jvm-call-await' (async `jvm-call-async' plus a blocking wait on the
worker), this runs the call on the Emacs main thread itself, with no
worker round trip. It blocks Emacs's main loop for the whole call, so
keep heavy or blocking work on the Clojure side's own threads."
  (jvm--call-direct target member args))

(defun jvm-new-direct (class-name args)
  "Construct CLASS-NAME(ARGS...) synchronously on this thread.
Sugar over `jvm-call-direct' with MEMBER `\"<init>\"'."
  (jvm-call-direct class-name "<init>" args))

(defalias 'jvm-call 'jvm-call-direct
  "Canonical `jvm-call' is the direct (main-thread) variant. See
`jvm-call-async' for the non-blocking form and `jvm-call-await' for the
worker-blocking form.")

(defalias 'jvm-new 'jvm-new-direct
  "Canonical `jvm-new' is the direct (main-thread) variant. See `jvm-new-async'
for the non-blocking form and `jvm-new-await' for the worker-blocking form.")

(defvar jvm--lisp-callbacks (make-hash-table :test 'eql)
  "Handle -> Lisp function, for functions passed to Java as `LispFunction'.
`jvm--drop-callback' removes an entry when the Java proxy is collected.")

(defvar jvm--next-callback-handle 1
  "Monotonic counter for `jvm--lisp-callbacks' keys.")

(defun jvm--register-callback (fn)
  "Register FN in `jvm--lisp-callbacks', returning its fresh handle."
  (let ((handle jvm--next-callback-handle))
    (setq jvm--next-callback-handle (1+ jvm--next-callback-handle))
    (puthash handle fn jvm--lisp-callbacks)
    handle))

(provide 'jvm)
;;; jvm.el ends here
