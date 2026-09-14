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
Only honored on a genuinely fresh `jvm-start' (from `not-started',
`stopped', `start-failed', `stop-failed', or `crashed'). Resuming from
`paused' reuses the already-running JVM and ignores this variable, since
JVM options cannot change after the JVM is created.")

(defvar jvm-options '("-Xrs")
  "List of JVM flag strings (e.g. \"-Xmx512m\") passed at `jvm-start' time.
Same first-start-only caveat as `jvm-classpath'.

Defaults to `(\"-Xrs\")': without it, the JVM installs its own handlers for
SIGINT/SIGTERM/SIGHUP/SIGQUIT (to support `Runtime.addShutdownHook' and a
SIGQUIT thread dump), which — since the JVM shares Emacs's process — means
`C-g' can terminate Emacs outright instead of just interrupting Emacs.
`-Xrs' stops the JVM from installing those handlers at all, leaving
whatever Emacs itself does with those signals untouched; this has no
downside for this module, since `jvm-stop' always tears the JVM down via
the JNI API (`JavaVM::destroy'), not via a signal, and shutdown hooks still
run on that path with or without `-Xrs'. If you override this variable,
keep \"-Xrs\" in the list unless you specifically want the JVM's signal
handlers installed (e.g. to use `kill -QUIT' for a thread dump — see
docs/explanation/architecture.md).")

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

(defvar jvm--pending-call nil
  "Alist of request-id -> callback for in-flight `jvm-call'/`jvm-new' calls.")

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
    (`(call-done ,id ,ok ,result ,err)
     (when-let ((callback (alist-get id jvm--pending-call)))
       (setq jvm--pending-call (assq-delete-all id jvm--pending-call))
       (funcall callback ok (jvm--resolve-sexp-result result) err)))
    (`(invoke ,id ,handle ,args)
     ;; A JVM-initiated call into a previously wrapped Lisp function (see
     ;; `jvm--register-callback'): look it up by HANDLE, funcall it with
     ;; ARGS, and reply with the result (or a formatted error) via
     ;; `jvm--invoke-reply', which resolves the `CompletableFuture' on the
     ;; Rust/Java side.
     ;;
     ;; ARGS can contain a `(jvm-handle N)' tag, exactly like a `call-done'
     ;; RESULT can: any real Java object passed as one of the call's
     ;; arguments (not just an object that started out as a Lisp value and
     ;; crossed into Java earlier) renders this way -- see
     ;; `marshal::jvm_value_to_lisp_arg' and `shim.rs''s
     ;; `jobject_array_to_sexp'. So each argument must go through
     ;; `jvm--resolve-sexp-result' first, the same as `call-done' does,
     ;; to turn any such tag into a real `user-ptr' before FN sees it.
     (let ((fn (gethash handle jvm--lisp-callbacks)))
       (if (null fn)
           (jvm--invoke-reply id nil (format "no such callback handle %s" handle))
         (condition-case err
             (jvm--invoke-reply id t (apply fn (mapcar #'jvm--resolve-sexp-result args)))
           (error
            (jvm--invoke-reply id nil (format "%S" err)))))))
    (`(lisp-request)
     ;; A JVM thread enqueued one or more Lisp-access requests (`intern',
     ;; `apply', `type-of', `is-not-nil?', `eq', `prin1-to-string', or a
     ;; GC-driven `FreeRef') on the Rust-side queue and woke us up to
     ;; service them -- see `jvm--service-lisp-requests' and the design
     ;; spec's "Request/reply flow". No payload: the request itself rides
     ;; the Rust queue, not this message.
     (jvm--service-lisp-requests))
    (`(release ,handle)
     ;; The JVM-side LispFunction proxy for HANDLE was garbage collected
     ;; (see `ReleaseNotifier' in java/emacs_jni and its native `run',
     ;; registered from `marshal.rs''s `register_release_on_gc'): drop our
     ;; own function-table entry so it doesn't outlive the proxy that
     ;; referenced it. A no-op if HANDLE is already gone.
     (remhash handle jvm--lisp-callbacks))
    (_ (message "emacs-jni: unrecognized channel message: %S" sexp))))

(defun jvm--resolve-sexp-result (result)
  "Convert RESULT as parsed from the wire (may contain `(jvm-handle N)')
into its final Lisp-visible form."
  (pcase result
    (`(jvm-handle ,n) (jvm--wrap-handle n))
    (_ result)))

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
    ;; Multiple Lisp threads may independently call a `-sync' wrapper;
    ;; without this, any thread other than whichever one created the
    ;; process hard-errors on its first `accept-process-output' (verified
    ;; empirically — see .probe/lisp-thread-filter-probe/).
    (set-process-thread jvm--pipe-process nil)
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

(defun jvm-call (target member args &optional callback)
  "Call TARGET.MEMBER(ARGS...) on the JVM asynchronously.
TARGET is a class-name string (static call) or a wrapped JVM object
\(instance call). ARGS is a list. Returns a request id immediately. If
CALLBACK is given, it is called as (CALLBACK OK RESULT ERR) once the call
finishes."
  (let ((id (jvm--call target member args)))
    (when callback
      (push (cons id callback) jvm--pending-call))
    id))

(defun jvm-call-sync (target member args &optional timeout)
  "Like `jvm-call', but blocks until it finishes or TIMEOUT (default 30s).
Returns (OK RESULT ERR) as a 3-element list; signals `jvm-timeout' if
TIMEOUT elapses first."
  (let* ((result :pending)
         (id (jvm-call target member args
                        (lambda (ok res err) (setq result (list ok res err)))))
         (deadline (+ (float-time) (or timeout 30))))
    (while (eq result :pending)
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (setq jvm--pending-call (assq-delete-all id jvm--pending-call))
          (signal 'jvm-timeout (list 'jvm-call-sync target member)))))
    result))

(defun jvm-new (class-name args &optional callback)
  "Construct a new CLASS-NAME(ARGS...) instance on the JVM asynchronously.
Sugar over `jvm-call' with MEMBER set to the constructor's actual internal
JVM bytecode name, `\"<init>\"'."
  (jvm-call class-name "<init>" args callback))

(defun jvm-new-sync (class-name args &optional timeout)
  "Like `jvm-new', but blocks. See `jvm-call-sync'."
  (jvm-call-sync class-name "<init>" args timeout))

(defun jvm-call-direct (target member args)
  "Call TARGET.MEMBER(ARGS...) on the JVM synchronously, on this thread.
Returns the result value directly; signals `jvm-error' on failure.

Unlike `jvm-call-sync' (async `jvm-call' plus a blocking wait on the
worker), this runs the call on the Emacs main thread itself, with no
worker round trip. It blocks Emacs's main loop for the whole call, so
keep heavy or blocking work on the Clojure side's own threads."
  (jvm--call-direct target member args))

(defun jvm-new-direct (class-name args)
  "Construct CLASS-NAME(ARGS...) synchronously on this thread.
Sugar over `jvm-call-direct' with MEMBER `\"<init>\"'."
  (jvm-call-direct class-name "<init>" args))

(defvar jvm--lisp-callbacks (make-hash-table :test 'eql)
  "Handle -> Lisp function, for functions wrapped as JVM LispFunction
proxies. Entries are removed when the JVM notifies us via `(release
HANDLE)' that the corresponding proxy was garbage collected.")

(defvar jvm--next-callback-handle 1
  "Monotonic counter for `jvm--lisp-callbacks' keys.")

(defun jvm--register-callback (fn)
  "Register FN in `jvm--lisp-callbacks', returning its fresh handle."
  (let ((handle jvm--next-callback-handle))
    (setq jvm--next-callback-handle (1+ jvm--next-callback-handle))
    (puthash handle fn jvm--lisp-callbacks)
    handle))

(defun jvm-clojure--var (ns name)
  "Resolve the wrapped IFn for NS/NAME via `clojure.java.api.Clojure/var'.
Returns (OK IFN ERR), same shape as `jvm-call-sync'."
  (jvm-call-sync "clojure.java.api.Clojure" "var" (list ns name)))

(defun jvm-clojure--read (form-string)
  "Parse FORM-STRING as Clojure data via `clojure.java.api.Clojure/read'.
Returns (OK DATA ERR), same shape as `jvm-call-sync'."
  (jvm-call-sync "clojure.java.api.Clojure" "read" (list form-string)))

(defun jvm-clojure--invoke-with-form (var-ns var-name form-string &optional timeout)
  "Resolve VAR-NS/VAR-NAME, read FORM-STRING as Clojure data, and invoke
the resolved IFn with it as its sole argument. Returns (OK RESULT ERR) --
whichever step fails first short-circuits the rest. Shared plumbing for
`jvm-clojure-require' and `jvm-clojure-eval-string', both of which are
\"resolve a var, read one form, invoke\"."
  (let ((fn (jvm-clojure--var var-ns var-name)))
    (if (not (car fn))
        fn
      (let ((form (jvm-clojure--read form-string)))
        (if (not (car form))
            form
          (jvm-call-sync (nth 1 fn) "invoke" (list (nth 1 form)) timeout))))))

(defun jvm-clojure-require (namespace &optional timeout)
  "Require the Clojure NAMESPACE (a string, e.g. \"clojure.set\").
Returns (OK RESULT ERR), same shape as `jvm-call-sync'."
  (jvm-clojure--invoke-with-form "clojure.core" "require" namespace timeout))

(defun jvm-clojure-eval-string (code &optional timeout)
  "Evaluate CODE (a string of Clojure source) and return its result.
Returns (OK RESULT ERR), same shape as `jvm-call-sync'.

Each call may run on a different underlying JVM thread (Phase 2's
per-call dispatch model), so `*ns*' set by a previous call (e.g. via
`in-ns') does not persist to the next one -- fold namespace switches into
CODE itself when it matters. See
docs/superpowers/specs/2026-09-02-clojure-nrepl-design.md, \"Known
limitation\". Non-scalar Clojure return values (keywords, vectors, maps)
come back as an opaque wrapped JVM object, same limitation as any other
non-scalar `jvm-call' result."
  (jvm-clojure--invoke-with-form "clojure.core" "eval" code timeout))

(defun jvm-clojure--invoke-with-form-direct (var-ns var-name form-string)
  "Direct (main-thread, synchronous) counterpart of
`jvm-clojure--invoke-with-form': resolve VAR-NS/VAR-NAME, read FORM-STRING,
and invoke the resolved IFn with it, every step via `jvm-call-direct'. Returns
the result value directly (not an (OK RESULT ERR) tuple); a failure signals
`jvm-error', and a Lisp signal/throw from a reentrant callback propagates out
as itself. Because the main thread stays attached across these calls, thread-
locals (e.g. `*ns*') set by one persist into the next."
  (let* ((fn (jvm-call-direct "clojure.java.api.Clojure" "var" (list var-ns var-name)))
         (form (jvm-call-direct "clojure.java.api.Clojure" "read" (list form-string))))
    (jvm-call-direct fn "invoke" (list form))))

(defun jvm-clojure-require-direct (namespace)
  "Direct counterpart of `jvm-clojure-require'. Requires the Clojure
NAMESPACE synchronously on this thread; returns the result value directly and
signals `jvm-error' on failure."
  (jvm-clojure--invoke-with-form-direct "clojure.core" "require" namespace))

(defun jvm-clojure-eval-string-direct (code)
  "Direct counterpart of `jvm-clojure-eval-string': evaluate CODE (a string of
Clojure source) synchronously on the Emacs main thread and return its result
value directly (signalling `jvm-error' on failure), with no worker round trip.

Unlike `jvm-clojure-eval-string', successive calls run on the *same*, always-
attached JVM thread, so a thread-local effect (e.g. `*ns*' after `in-ns', or a
dynamic binding) does persist across calls. It blocks Emacs's main loop for the
whole eval, so keep heavy or blocking work on the Clojure side's own threads."
  (jvm-clojure--invoke-with-form-direct "clojure.core" "eval" code))

(defun jvm-nrepl-start (&optional port bind timeout)
  "Start an nREPL server (requiring `nrepl.server' first if needed).
PORT defaults to 0, letting the OS pick a free port. BIND, if non-nil,
is the interface address string the server listens on (e.g. \"0.0.0.0\"
for all interfaces); it defaults to nREPL's own default, loopback. Bind
beyond loopback only when you need it -- a loopback-only server is
unreachable through VM/NAT port forwarding, which delivers the
connection to a routable interface, not the guest's `127.0.0.1'. See
docs/how-to-guides/run-an-nrepl-server.md for that and the security
cost. Returns (OK RESULT ERR); if OK, RESULT is (cons ACTUAL-PORT
SERVER) -- the real bound port and a wrapped JVM object `jvm-nrepl-stop'
needs later."
  (catch 'jvm-nrepl-start
    (let ((required (jvm-clojure-require "nrepl.server" timeout)))
      (unless (car required) (throw 'jvm-nrepl-start required))
      (let ((start-fn (jvm-clojure--var "nrepl.server" "start-server")))
        (unless (car start-fn) (throw 'jvm-nrepl-start start-fn))
        (let ((port-kw (jvm-clojure--read ":port")))
          (unless (car port-kw) (throw 'jvm-nrepl-start port-kw))
          (let ((start-args (list (nth 1 port-kw) (or port 0))))
            ;; `start-server' takes keyword options; add `:bind BIND' to
            ;; its argument list only when a bind address was requested,
            ;; so the default path stays a plain `:port' call.
            (when bind
              (let ((bind-kw (jvm-clojure--read ":bind")))
                (unless (car bind-kw) (throw 'jvm-nrepl-start bind-kw))
                (setq start-args (append start-args (list (nth 1 bind-kw) bind)))))
            (let ((server (jvm-call-sync (nth 1 start-fn) "invoke" start-args timeout)))
              (unless (car server) (throw 'jvm-nrepl-start server))
              (let ((bound-port (jvm-call-sync (nth 1 port-kw) "invoke"
                                                (list (nth 1 server)) timeout)))
                (unless (car bound-port) (throw 'jvm-nrepl-start bound-port))
                (list t (cons (nth 1 bound-port) (nth 1 server)) nil)))))))))

(defun jvm-nrepl-stop (server &optional timeout)
  "Stop SERVER (the cdr of `jvm-nrepl-start''s RESULT). Returns
\(OK RESULT ERR), same shape as `jvm-call-sync'."
  (let ((stop-fn (jvm-clojure--var "nrepl.server" "stop-server")))
    (if (not (car stop-fn))
        stop-fn
      (jvm-call-sync (nth 1 stop-fn) "invoke" (list server) timeout))))

(provide 'jvm)
;;; jvm.el ends here
