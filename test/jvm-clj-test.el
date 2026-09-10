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
method call uses -- a Java `emacs_jni.core.LispObject' has no special status on
the Lisp side, it's just an opaque wrapped handle, per the design spec's
non-goal: \"marshalling arbitrary Lisp values to Clojure data\")."
  (let ((result (jvm-call-sync lisp-object "prin1ToString" nil)))
    (should (car result))
    (nth 1 result)))

(ert-deftest emacs-jni-jvm-clj-test/intern-returns-a-clojure-symbol ()
  (jvm-clj-test--ensure-ready)
  ;; intern now yields a Clojure symbol, not an opaque LispObject.
  (should (eq (jvm-clj-test--eval "(symbol? (dev.ublt.emacs.clj/intern \"car\"))") t))
  (should (equal (jvm-clj-test--eval "(str (dev.ublt.emacs.clj/intern \"car\"))") "car")))

(ert-deftest emacs-jni-jvm-clj-test/intern-colon-name-returns-a-clojure-keyword ()
  (jvm-clj-test--ensure-ready)
  ;; A leading ":" interns a Lisp keyword, which maps to a Clojure keyword.
  (should (eq (jvm-clj-test--eval "(keyword? (dev.ublt.emacs.clj/intern \":foo\"))") t))
  (should (equal (jvm-clj-test--eval "(name (dev.ublt.emacs.clj/intern \":foo\"))") "foo")))

(ert-deftest emacs-jni-jvm-clj-test/lisp-symbol-result-becomes-a-clojure-symbol ()
  (jvm-clj-test--ensure-ready)
  ;; Both directions at once: (symbol "intern") crosses Clojure->Lisp as
  ;; the Lisp function `intern', whose result (a Lisp symbol foo) crosses
  ;; back Lisp->Clojure as a Clojure symbol.
  (should (eq (jvm-clj-test--eval
               "(symbol? (dev.ublt.emacs.clj/apply (symbol \"intern\") \"foo\"))")
              t))
  (should (equal (jvm-clj-test--eval
                  "(str (dev.ublt.emacs.clj/apply (symbol \"intern\") \"foo\"))")
                 "foo")))

(ert-deftest emacs-jni-jvm-clj-test/clojure-symbol-arg-becomes-a-lisp-symbol ()
  (jvm-clj-test--ensure-ready)
  ;; (symbol "symbol-name") is the receiver, (symbol "car") the argument;
  ;; both cross as Lisp symbols. (symbol-name 'car) => "car".
  (should (equal (jvm-clj-test--eval
                  "(dev.ublt.emacs.clj/apply (symbol \"symbol-name\") (symbol \"car\"))")
                 "car")))

(ert-deftest emacs-jni-jvm-clj-test/clojure-keyword-arg-becomes-a-lisp-keyword ()
  (jvm-clj-test--ensure-ready)
  ;; A Lisp keyword's `symbol-name' includes the leading colon.
  (should (equal (jvm-clj-test--eval
                  "(dev.ublt.emacs.clj/apply (symbol \"symbol-name\") (keyword \"foo\"))")
                 ":foo")))

(ert-deftest emacs-jni-jvm-clj-test/namespaced-symbol-round-trips-by-full-name ()
  (jvm-clj-test--ensure-ready)
  (should (equal (jvm-clj-test--eval
                  "(str (dev.ublt.emacs.clj/apply (symbol \"intern\") \"foo/bar\"))")
                 "foo/bar")))

(ert-deftest emacs-jni-jvm-clj-test/interned-symbol-round-trips-eq ()
  (jvm-clj-test--ensure-ready)
  ;; apply returns the Lisp symbol foo (as a Clojure symbol); eq'ing it
  ;; with a fresh (symbol "foo") re-interns to the same Lisp symbol.
  (should (eq (jvm-clj-test--eval
               "(dev.ublt.emacs.clj/eq (dev.ublt.emacs.clj/apply (symbol \"intern\") \"foo\")
                                        (symbol \"foo\"))")
              t)))

(ert-deftest emacs-jni-jvm-clj-test/uninterned-lisp-symbol-stays-a-lisp-object ()
  (jvm-clj-test--ensure-ready)
  ;; make-symbol yields an uninterned symbol; it is NOT converted to a
  ;; Clojure symbol (identity preserved), so `symbol?' is false and the
  ;; value stays an opaque LispObject.
  (should (eq (jvm-clj-test--eval
               "(symbol? (dev.ublt.emacs.clj/apply (symbol \"make-symbol\") \"uninterned-xyz\"))")
              nil))
  (let ((result (jvm-clj-test--eval
                 "(dev.ublt.emacs.clj/apply (symbol \"make-symbol\") \"uninterned-xyz\")")))
    (should (equal (jvm-clj-test--prin1 result) "uninterned-xyz"))))

(ert-deftest emacs-jni-jvm-clj-test/apply-integer-result-becomes-a-clojure-value ()
  (jvm-clj-test--ensure-ready)
  ;; `+' returns an integer -> a Clojure Long, not an opaque LispObject.
  ;; `jvm-clojure-eval-string' then marshals that Long back to Lisp as 3.
  (should (equal (jvm-clj-test--eval
                  "(dev.ublt.emacs.clj/apply (dev.ublt.emacs.clj/intern \"+\") 1 2)")
                 3)))

(ert-deftest emacs-jni-jvm-clj-test/apply-string-result-becomes-a-clojure-string ()
  (jvm-clj-test--ensure-ready)
  (should (equal (jvm-clj-test--eval
                  "(dev.ublt.emacs.clj/apply (dev.ublt.emacs.clj/intern \"concat\") \"a\" \"b\")")
                 "ab")))

(ert-deftest emacs-jni-jvm-clj-test/apply-float-result-becomes-a-clojure-double ()
  (jvm-clj-test--ensure-ready)
  (should (equal (jvm-clj-test--eval
                  "(dev.ublt.emacs.clj/apply (dev.ublt.emacs.clj/intern \"+\") 1.0 0.5)")
                 1.5)))

(ert-deftest emacs-jni-jvm-clj-test/apply-nil-result-becomes-clojure-nil ()
  (jvm-clj-test--ensure-ready)
  ;; (not 1) => nil, converted to Clojure nil, marshalled back as Lisp nil.
  (should (eq (jvm-clj-test--eval
               "(dev.ublt.emacs.clj/apply (dev.ublt.emacs.clj/intern \"not\") 1)")
              nil)))

(ert-deftest emacs-jni-jvm-clj-test/apply-t-result-becomes-clojure-true ()
  (jvm-clj-test--ensure-ready)
  ;; (not nil) => t, converted to Clojure true, marshalled back as Lisp t.
  (should (eq (jvm-clj-test--eval
               "(dev.ublt.emacs.clj/apply (dev.ublt.emacs.clj/intern \"not\") nil)")
              t)))

(ert-deftest emacs-jni-jvm-clj-test/apply-non-scalar-result-stays-a-lisp-object ()
  (jvm-clj-test--ensure-ready)
  ;; A list is not a scalar, so it stays an opaque LispObject; read it
  ;; back with prin1ToString.
  (let ((result (jvm-clj-test--eval
                 "(dev.ublt.emacs.clj/apply (dev.ublt.emacs.clj/intern \"list\") 1 2 3)")))
    (should (equal (jvm-clj-test--prin1 result) "(1 2 3)"))))

(ert-deftest emacs-jni-jvm-clj-test/apply-lisp-error-completes-the-future-exceptionally ()
  "A Lisp `error' during `apply' completes the future exceptionally with
`emacs_jni.core.LispError', so `deref' throws it (wrapped in the
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
                 (if (instance? emacs_jni.core.LispError (.getCause e))
                   \"lisp-error\"
                   (str \"other-cause:\" (.getName (.getClass (.getCause e))))))
               (catch emacs_jni.core.LispError e \"lisp-error\")
               (catch Throwable e (str \"other:\" (.getName (.getClass e)))))")
           "lisp-error")))

(ert-deftest emacs-jni-jvm-clj-test/type-of-returns-a-clojure-symbol ()
  (jvm-clj-test--ensure-ready)
  ;; type-of yields the Lisp type symbol as a Clojure symbol (e.g.
  ;; `string' for a string value).
  (should (eq (jvm-clj-test--eval "(symbol? (dev.ublt.emacs.clj/type-of \"hello\"))") t))
  (should (equal (jvm-clj-test--eval "(str (dev.ublt.emacs.clj/type-of \"hello\"))") "string")))

(ert-deftest emacs-jni-jvm-clj-test/eq-is-true-for-the-same-symbol ()
  (jvm-clj-test--ensure-ready)
  (should (eq (jvm-clj-test--eval
               "(dev.ublt.emacs.clj/eq (symbol \"car\") (symbol \"car\"))")
              t)))

(ert-deftest emacs-jni-jvm-clj-test/eq-is-false-for-different-symbols ()
  (jvm-clj-test--ensure-ready)
  (should (eq (jvm-clj-test--eval
               "(dev.ublt.emacs.clj/eq (symbol \"car\") (symbol \"cdr\"))")
              nil)))

(ert-deftest emacs-jni-jvm-clj-test/is-not-nil-is-true-for-a-non-nil-symbol ()
  (jvm-clj-test--ensure-ready)
  (should (eq (jvm-clj-test--eval
               "(dev.ublt.emacs.clj/is-not-nil? (symbol \"car\"))")
              t)))

(ert-deftest emacs-jni-jvm-clj-test/is-not-nil-is-false-for-nil ()
  (jvm-clj-test--ensure-ready)
  (should (eq (jvm-clj-test--eval
               "(dev.ublt.emacs.clj/is-not-nil? nil)")
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
               "(let [sym (dev.ublt.emacs.clj/intern \"car\")]
                  (dev.ublt.emacs.clj/is-not-nil? sym))")
              t)))

(provide 'jvm-clj-test)
;;; jvm-clj-test.el ends here
