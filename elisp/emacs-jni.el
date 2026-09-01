;;; emacs-jni.el --- JVM embedding for Emacs -*- lexical-binding: t; -*-

(defvar emacs-jni--module-path
  (or (getenv "EMACS_JNI_MODULE_PATH")
      (expand-file-name "../target/debug/libemacs_jni.so"
                         (file-name-directory (or load-file-name buffer-file-name))))
  "Path to the built emacs-jni dynamic module (.so file).")

(unless (featurep 'emacs-jni)
  (module-load emacs-jni--module-path))

(defvar jvm-classpath nil
  "List of paths added to the JVM's classpath when it starts.
Only honored on a genuinely fresh `jvm-start' (from `not-started',
`stopped', `start-failed', `stop-failed', or `crashed'). Resuming from
`paused' reuses the already-running JVM and ignores this variable, since
JVM options cannot change after the JVM is created.")

(defvar jvm-options nil
  "List of JVM flag strings (e.g. \"-Xmx512m\") passed at `jvm-start' time.
Same first-start-only caveat as `jvm-classpath'.")

(defvar jvm--status 'not-started
  "Current JVM lifecycle status. See `jvm-status'.")

(defvar jvm-status-hook nil
  "Hook run with the new status symbol on every JVM status transition.")

(defvar jvm--pipe-process nil
  "The pipe process used to receive messages from the JVM worker thread.")

(defvar jvm--filter-buffer ""
  "Accumulates partial output from `jvm--pipe-process' between newlines.")

(defvar jvm--pending-load-jar nil
  "Alist of request-id -> callback for in-flight `jvm-load-jar' calls.")

(defvar jvm--log-buffer-name "*jvm-log*"
  "Name of the buffer that accumulates JVM log and status-failure messages.")

(defun jvm--log (format-string &rest args)
  "Append a line to the `*jvm-log*' buffer, formatted like `format'."
  (with-current-buffer (get-buffer-create jvm--log-buffer-name)
    (goto-char (point-max))
    (insert (apply #'format format-string args) "\n")))

(defun jvm--dispatch-message (sexp)
  "Handle one parsed message SEXP from the JVM worker thread."
  (pcase sexp
    (`(status ,state ,msg)
     (setq jvm--status state)
     (when msg
       (jvm--log "[status] %s: %s" state msg))
     (run-hook-with-args 'jvm-status-hook state))
    (`(log ,level ,msg)
     (jvm--log "[%s] %s" level msg)
     (message "emacs-jni [%s]: %s" level msg))
    (`(load-jar-done ,id ,ok ,err)
     (when-let ((callback (alist-get id jvm--pending-load-jar)))
       (setq jvm--pending-load-jar (assq-delete-all id jvm--pending-load-jar))
       (funcall callback ok err)))
    (_ (message "emacs-jni: unrecognized channel message: %S" sexp))))

(defun jvm--process-filter (_proc data)
  "Accumulate DATA and dispatch each complete newline-terminated S-expression."
  (setq jvm--filter-buffer (concat jvm--filter-buffer data))
  (let (newline-pos)
    (while (setq newline-pos (string-search "\n" jvm--filter-buffer))
      (let ((line (substring jvm--filter-buffer 0 newline-pos)))
        (setq jvm--filter-buffer (substring jvm--filter-buffer (1+ newline-pos)))
        (unless (string-empty-p line)
          (jvm--dispatch-message (read line)))))))

(defun jvm--ensure-pipe-process ()
  "Create the pipe process and open the Rust-side channel, once."
  (unless (process-live-p jvm--pipe-process)
    (setq jvm--pipe-process
          (make-pipe-process
           :name "emacs-jni-channel"
           :filter #'jvm--process-filter
           :noquery t))
    (jvm--init-channel jvm--pipe-process)))

(defun jvm-status ()
  "Return the current JVM lifecycle status symbol."
  jvm--status)

(defun jvm-start (&optional classpath options)
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

(defun jvm-start-sync (&optional classpath options timeout)
  "Like `jvm-start', but blocks until the JVM is ready or TIMEOUT (default 30s) elapses."
  (jvm-start classpath options)
  (let ((deadline (+ (float-time) (or timeout 30))))
    (while (eq (jvm-status) 'starting)
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (signal 'jvm-timeout (list 'jvm-start-sync))))))
  (jvm-status))

(defun jvm-pause ()
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

(defun jvm-pause-sync (&optional timeout)
  "Like `jvm-pause', but blocks until paused or TIMEOUT (default 30s) elapses."
  (jvm-pause)
  (let ((deadline (+ (float-time) (or timeout 30))))
    (while (eq (jvm-status) 'pausing)
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (signal 'jvm-timeout (list 'jvm-pause-sync))))))
  (jvm-status))

(defun jvm-stop ()
  "Attempt to fully tear down the JVM asynchronously. Experimental: this
relies on JNI_DestroyJavaVM, which some JVM builds handle unreliably (see
the design spec's Risks section). `jvm-start' afterward is best-effort.
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

(defun jvm-stop-sync (&optional timeout)
  "Like `jvm-stop', but blocks until stopped/stop-failed or TIMEOUT (default 30s)."
  (jvm-stop)
  (let ((deadline (+ (float-time) (or timeout 30))))
    (while (eq (jvm-status) 'stopping)
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (signal 'jvm-timeout (list 'jvm-stop-sync))))))
  (jvm-status))

(defun jvm-load-jar (path &optional callback)
  "Load the jar at PATH into the shared classloader asynchronously.
Returns a request id immediately. If CALLBACK is given, it is called as
\(CALLBACK OK ERR) once loading finishes."
  (let ((id (jvm--load-jar path)))
    (when callback
      (push (cons id callback) jvm--pending-load-jar))
    id))

(defun jvm-load-jar-sync (path &optional timeout)
  "Like `jvm-load-jar', but blocks until it finishes or TIMEOUT (default 30s).
Returns a cons (OK . ERR); signals `jvm-timeout' if TIMEOUT elapses first."
  (let* ((result :pending)
         (id (jvm-load-jar path (lambda (ok err) (setq result (cons ok err)))))
         (deadline (+ (float-time) (or timeout 30))))
    (while (eq result :pending)
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (setq jvm--pending-load-jar (assq-delete-all id jvm--pending-load-jar))
          (signal 'jvm-timeout (list 'jvm-load-jar-sync path)))))
    result))

(provide 'emacs-jni)
;;; emacs-jni.el ends here
