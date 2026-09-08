;;; jvm-clj-test.el --- Tests for built-in Lisp access from Clojure -*- lexical-binding: t; -*-

(require 'ert)
(require 'emacs-jni)
(require 'jvm-clj)

(defvar jvm-clj-test--jars-dir (getenv "EMACS_JNI_CLOJURE_JARS_DIR")
  "Directory containing Clojure/nREPL jars, or nil if unset. See
`jvm-clojure-test--jars-dir' in test/jvm-clojure-test.el for why tests in
this file skip cleanly (not fail) when this isn't configured.")

(defvar jvm-clj-test--ready nil
  "Set to t once the JVM is started and `dev.ublt.emacs.clj' is loaded, so
every test in this file can call `jvm-clj-test--ensure-ready' idempotently
regardless of ERT's execution order.")

(defun jvm-clj-test--skip-unless-jars ()
  (unless (and jvm-clj-test--jars-dir (file-directory-p jvm-clj-test--jars-dir))
    (ert-skip "Set EMACS_JNI_CLOJURE_JARS_DIR (see test/fetch-clojure-jars.sh) to run this test")))

(defun jvm-clj-test--ensure-ready ()
  (jvm-clj-test--skip-unless-jars)
  (unless jvm-clj-test--ready
    (setq jvm-clj-classpath (directory-files jvm-clj-test--jars-dir t "\\.jar\\'"))
    (jvm-clj-start)
    (setq jvm-clj-test--ready t)))

(defun jvm-clj-test--eval (code)
  "Evaluate CODE (Clojure source) via `jvm-clojure-eval-string', asserting
success, and return its RESULT."
  (let ((result (jvm-clojure-eval-string code)))
    (should (car result))
    (nth 1 result)))

(defun jvm-clj-test--prin1 (lisp-object)
  "Call `prin1ToString' on the wrapped Java LISP-OBJECT via plain
`jvm-call-sync' reflection (the same generic path any other JVM-object
method call uses -- a Java `emacs_jni.LispObject' has no special status on
the Lisp side, it's just an opaque wrapped handle, per the design spec's
non-goal: \"marshalling arbitrary Lisp values to Clojure data\")."
  (let ((result (jvm-call-sync lisp-object "prin1ToString" nil)))
    (should (car result))
    (nth 1 result)))

(ert-deftest emacs-jni-jvm-clj-test/intern-returns-a-lisp-object ()
  (jvm-clj-test--ensure-ready)
  (let ((sym (jvm-clj-test--eval "(dev.ublt.emacs.clj/intern \"car\")")))
    (should (equal (jvm-clj-test--prin1 sym) "car"))))

(ert-deftest emacs-jni-jvm-clj-test/apply-plus-returns-a-lisp-object-whose-prin1-is-3 ()
  (jvm-clj-test--ensure-ready)
  (let ((result (jvm-clj-test--eval
                 "(dev.ublt.emacs.clj/apply (dev.ublt.emacs.clj/intern \"+\") 1 2)")))
    (should (equal (jvm-clj-test--prin1 result) "3"))))

(ert-deftest emacs-jni-jvm-clj-test/apply-lisp-error-completes-the-future-exceptionally ()
  "A Lisp `error' during `apply' completes the future exceptionally with
`emacs_jni.LispError', so `deref' throws it (wrapped in the
`CompletableFuture'-standard `ExecutionException'). Design spec flags
this path as load-bearing (\"A Lisp error becomes an exceptional future
completion with LispError, which surfaces on deref\"). `apply' derefs
`apply-async's future internally, so it raises that exceptional
completion here. Asserts on the exception TYPE, not its message text,
since the message is the Rust-side `Display' of an `emacs::Error' -- an
implementation detail, unlike the LispError contract the JVM caller
actually catches on."
  (jvm-clj-test--ensure-ready)
  (should (equal
           (jvm-clj-test--eval
            "(try
               (dev.ublt.emacs.clj/apply (dev.ublt.emacs.clj/intern \"error\") \"boom\")
               \"no-throw\"
               (catch java.util.concurrent.ExecutionException e
                 (if (instance? emacs_jni.LispError (.getCause e))
                   \"lisp-error\"
                   (str \"other-cause:\" (.getName (.getClass (.getCause e))))))
               (catch emacs_jni.LispError e \"lisp-error\")
               (catch Throwable e (str \"other:\" (.getName (.getClass e)))))")
           "lisp-error")))

(ert-deftest emacs-jni-jvm-clj-test/type-of-a-symbol-is-the-symbol-type ()
  (jvm-clj-test--ensure-ready)
  (let ((result (jvm-clj-test--eval
                 "(dev.ublt.emacs.clj/type-of (dev.ublt.emacs.clj/intern \"car\"))")))
    (should (equal (jvm-clj-test--prin1 result) "symbol"))))

(ert-deftest emacs-jni-jvm-clj-test/eq-is-true-for-the-same-interned-symbol ()
  (jvm-clj-test--ensure-ready)
  (should (eq (jvm-clj-test--eval
               "(dev.ublt.emacs.clj/eq (dev.ublt.emacs.clj/intern \"car\") (dev.ublt.emacs.clj/intern \"car\"))")
              t)))

(ert-deftest emacs-jni-jvm-clj-test/eq-is-false-for-different-symbols ()
  (jvm-clj-test--ensure-ready)
  (should (eq (jvm-clj-test--eval
               "(dev.ublt.emacs.clj/eq (dev.ublt.emacs.clj/intern \"car\") (dev.ublt.emacs.clj/intern \"cdr\"))")
              nil)))

(ert-deftest emacs-jni-jvm-clj-test/is-not-nil-is-true-for-a-non-nil-symbol ()
  (jvm-clj-test--ensure-ready)
  (should (eq (jvm-clj-test--eval
               "(dev.ublt.emacs.clj/is-not-nil? (dev.ublt.emacs.clj/intern \"car\"))")
              t)))

(ert-deftest emacs-jni-jvm-clj-test/is-not-nil-is-false-for-the-nil-symbol ()
  (jvm-clj-test--ensure-ready)
  (should (eq (jvm-clj-test--eval
               "(dev.ublt.emacs.clj/is-not-nil? (dev.ublt.emacs.clj/intern \"nil\"))")
              nil)))

(ert-deftest emacs-jni-jvm-clj-test/clojure-calls-lisp-back-inside-a-lisp-to-clojure-call ()
  "Reentrancy: this test's OWN call is Lisp -> Clojure (`jvm-clojure-eval-
string', serviced by `jvm-call-sync's `accept-process-output' loop). The
Clojure code below itself calls back into Lisp twice (`apply', then
`is-not-nil?' on the result) *during* that same outer call -- each inner
round trip sends its own `(lisp-request)' wakeup, serviced by the SAME
still-running outer `accept-process-output' loop, before the outer
`eval' itself returns. See design spec, \"Core constraint\": \"a
`clj/apply' issued from inside a `jvm-clojure-eval-string' is serviced by
the outer `-sync' wrapper's own `accept-process-output' loop.\""
  (jvm-clj-test--ensure-ready)
  (should (eq (jvm-clj-test--eval
               "(let [result (dev.ublt.emacs.clj/apply (dev.ublt.emacs.clj/intern \"+\") 1 2)]
                  (dev.ublt.emacs.clj/is-not-nil? result))")
              t)))

(provide 'jvm-clj-test)
;;; jvm-clj-test.el ends here
