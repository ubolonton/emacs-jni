;;; jvm-clj.el --- Clojure access on top of the embedded JVM -*- lexical-binding: t; -*-

(require 'jvm)

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
function called later -- see `jvm--module-path' for the same
pattern).")

;;; Clojure interop primitives (async worker path).

(defun jvm-clj--var (ns name)
  "Resolve the wrapped IFn for NS/NAME via `clojure.java.api.Clojure/var'.
Returns (OK IFN ERR), same shape as `jvm-call-sync'."
  (jvm-call-sync "clojure.java.api.Clojure" "var" (list ns name)))

(defun jvm-clj--read (form-string)
  "Parse FORM-STRING as Clojure data via `clojure.java.api.Clojure/read'.
Returns (OK DATA ERR), same shape as `jvm-call-sync'."
  (jvm-call-sync "clojure.java.api.Clojure" "read" (list form-string)))

(defun jvm-clj--invoke-with-form (var-ns var-name form-string &optional timeout)
  "Resolve VAR-NS/VAR-NAME, read FORM-STRING as Clojure data, and invoke
the resolved IFn with it as its sole argument. Returns (OK RESULT ERR) --
whichever step fails first short-circuits the rest. Shared plumbing for
`jvm-clj-require' and `jvm-clj-eval-string', both of which are
\"resolve a var, read one form, invoke\"."
  (let ((fn (jvm-clj--var var-ns var-name)))
    (if (not (car fn))
        fn
      (let ((form (jvm-clj--read form-string)))
        (if (not (car form))
            form
          (jvm-call-sync (nth 1 fn) "invoke" (list (nth 1 form)) timeout))))))

(defun jvm-clj-require (namespace &optional timeout)
  "Require the Clojure NAMESPACE (a string, e.g. \"clojure.set\").
Returns (OK RESULT ERR), same shape as `jvm-call-sync'."
  (jvm-clj--invoke-with-form "clojure.core" "require" namespace timeout))

(defun jvm-clj-eval-string (code &optional timeout)
  "Evaluate CODE (a string of Clojure source) and return its result.
Returns (OK RESULT ERR), same shape as `jvm-call-sync'.

Each call may run on a different underlying JVM thread (the async per-call
dispatch model), so `*ns*' set by a previous call (e.g. via `in-ns') does
not persist to the next one -- fold namespace switches into CODE itself
when it matters, or use `jvm-clj-eval-string-direct', which keeps one
attached thread so thread-locals do persist. Non-scalar Clojure return
values (keywords, vectors, maps) come back as an opaque wrapped JVM object,
same limitation as any other non-scalar `jvm-call' result."
  (jvm-clj--invoke-with-form "clojure.core" "eval" code timeout))

;;; Clojure interop primitives (direct main-thread path).
;;;
;;; Each step goes through `jvm-call-direct': the call runs on the Emacs
;;; main thread, which stays attached to the JVM across calls, so successive
;;; direct calls share one JVM thread (stable identity, persistent thread-
;;; locals) and a Clojure->Lisp callback can reenter Lisp without deadlock.
;;; A failure signals `jvm-error'; a Lisp signal/throw raised in a reentrant
;;; callback propagates out as itself.

(defun jvm-clj--var-direct (ns name)
  "Direct counterpart of `jvm-clj--var': resolve the wrapped IFn for NS/NAME.
Returns the IFn value directly; signals `jvm-error' on failure."
  (jvm-call-direct "clojure.java.api.Clojure" "var" (list ns name)))

(defun jvm-clj--read-direct (form-string)
  "Direct counterpart of `jvm-clj--read': parse FORM-STRING as Clojure data.
Returns the data value directly; signals `jvm-error' on failure."
  (jvm-call-direct "clojure.java.api.Clojure" "read" (list form-string)))

(defun jvm-clj--invoke-with-form-direct (var-ns var-name form-string)
  "Direct counterpart of `jvm-clj--invoke-with-form': resolve VAR-NS/VAR-NAME,
read FORM-STRING, and invoke the resolved IFn with it, every step via
`jvm-call-direct'. Returns the result value directly."
  (let* ((fn (jvm-clj--var-direct var-ns var-name))
         (form (jvm-clj--read-direct form-string)))
    (jvm-call-direct fn "invoke" (list form))))

(defun jvm-clj-require-direct (namespace)
  "Direct counterpart of `jvm-clj-require'. Requires the Clojure NAMESPACE
synchronously on this thread; returns the result value directly and signals
`jvm-error' on failure."
  (jvm-clj--invoke-with-form-direct "clojure.core" "require" namespace))

(defun jvm-clj-eval-string-direct (code)
  "Direct counterpart of `jvm-clj-eval-string': evaluate CODE (a string of
Clojure source) synchronously on the Emacs main thread and return its result
value directly (signalling `jvm-error' on failure), with no worker round trip.

Unlike `jvm-clj-eval-string', successive calls run on the *same*, always-
attached JVM thread, so a thread-local effect (e.g. `*ns*' after `in-ns', or
a dynamic binding) does persist across calls. It blocks Emacs's main loop for
the whole eval, so keep heavy or blocking work on the Clojure side's own
threads."
  (jvm-clj--invoke-with-form-direct "clojure.core" "eval" code))

;;; Startup.

(defun jvm-clj-start (&optional timeout)
  "Start the JVM (if needed), load `jvm-clj-classpath' plus this
package's `clojure-src' directory, and require `emacs-jni.clj'.
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
  (let ((required (jvm-clj-require "emacs-jni.clj" timeout)))
    (unless (car required)
      (error "jvm-clj-start: failed to require emacs-jni.clj: %s" (nth 2 required)))
    required))

(defun jvm-clj-start-direct (&optional timeout)
  "Like `jvm-clj-start', but require `emacs-jni.clj' through the direct
\(main-thread) path (`jvm-clj-require-direct'). JVM start and jar loading
stay synchronous -- lifecycle/setup has no direct form. Mostly for symmetry:
`require' loads the namespace into the process-global runtime, so which
thread does it does not change the outcome. Returns the require result value
directly; signals `jvm-error' (or a lifecycle error) on failure."
  (unless (eq (jvm-status) 'ready)
    (unless (eq (jvm-start-sync nil nil timeout) 'ready)
      (error "jvm-clj-start-direct: JVM failed to start (status: %s)" (jvm-status))))
  (dolist (path (append jvm-clj-classpath (list jvm-clj--source-dir)))
    (let ((result (jvm-load-jar-sync path timeout)))
      (unless (car result)
        (error "jvm-clj-start-direct: failed to load %s: %s" path (cdr result)))))
  (jvm-clj-require-direct "emacs-jni.clj"))

;;; nREPL server. One layer: always ensures `emacs-jni.clj' is loaded first.

(defun jvm-clj-nrepl-start (&optional port bind timeout)
  "Ensure `emacs-jni.clj' is loaded (via `jvm-clj-start'), then start an
nREPL server (requiring `nrepl.server' first if needed).
PORT defaults to 0, letting the OS pick a free port. BIND, if non-nil,
is the interface address string the server listens on (e.g. \"0.0.0.0\"
for all interfaces); it defaults to nREPL's own default, loopback. Bind
beyond loopback only when you need it -- a loopback-only server is
unreachable through VM/NAT port forwarding, which delivers the
connection to a routable interface, not the guest's `127.0.0.1'. See
docs/how-to-guides/run-an-nrepl-server.md for that and the security
cost. Returns (OK RESULT ERR); if OK, RESULT is (cons ACTUAL-PORT
SERVER) -- the real bound port and a wrapped JVM object `jvm-clj-nrepl-stop'
needs later."
  (jvm-clj-start timeout)
  (catch 'jvm-clj-nrepl-start
    (let ((required (jvm-clj-require "nrepl.server" timeout)))
      (unless (car required) (throw 'jvm-clj-nrepl-start required))
      (let ((start-fn (jvm-clj--var "nrepl.server" "start-server")))
        (unless (car start-fn) (throw 'jvm-clj-nrepl-start start-fn))
        (let ((port-kw (jvm-clj--read ":port")))
          (unless (car port-kw) (throw 'jvm-clj-nrepl-start port-kw))
          (let ((start-args (list (nth 1 port-kw) (or port 0))))
            ;; `start-server' takes keyword options; add `:bind BIND' to
            ;; its argument list only when a bind address was requested,
            ;; so the default path stays a plain `:port' call.
            (when bind
              (let ((bind-kw (jvm-clj--read ":bind")))
                (unless (car bind-kw) (throw 'jvm-clj-nrepl-start bind-kw))
                (setq start-args (append start-args (list (nth 1 bind-kw) bind)))))
            (let ((server (jvm-call-sync (nth 1 start-fn) "invoke" start-args timeout)))
              (unless (car server) (throw 'jvm-clj-nrepl-start server))
              (let ((bound-port (jvm-call-sync (nth 1 port-kw) "invoke"
                                                (list (nth 1 server)) timeout)))
                (unless (car bound-port) (throw 'jvm-clj-nrepl-start bound-port))
                (list t (cons (nth 1 bound-port) (nth 1 server)) nil)))))))))

(defun jvm-clj-nrepl-stop (server &optional timeout)
  "Stop SERVER (the cdr of `jvm-clj-nrepl-start''s RESULT). Returns
\(OK RESULT ERR), same shape as `jvm-call-sync'."
  (let ((stop-fn (jvm-clj--var "nrepl.server" "stop-server")))
    (if (not (car stop-fn))
        stop-fn
      (jvm-call-sync (nth 1 stop-fn) "invoke" (list server) timeout))))

(defun jvm-clj-nrepl-start-direct (&optional port bind)
  "Direct (main-thread) counterpart of `jvm-clj-nrepl-start': ensure
`emacs-jni.clj' is loaded (via `jvm-clj-start-direct') and start the nREPL
server, routing every JVM/Clojure step through `jvm-call-direct'. PORT and
BIND are as in `jvm-clj-nrepl-start'. Returns (cons ACTUAL-PORT SERVER)
directly; signals `jvm-error' on failure."
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

(defun jvm-clj-nrepl-stop-direct (server)
  "Direct counterpart of `jvm-clj-nrepl-stop'. Stop SERVER synchronously on
this thread via `jvm-call-direct'; returns the result value directly and
signals `jvm-error' on failure."
  (jvm-call-direct (jvm-clj--var-direct "nrepl.server" "stop-server")
                   "invoke" (list server)))

(provide 'jvm-clj)
;;; jvm-clj.el ends here
