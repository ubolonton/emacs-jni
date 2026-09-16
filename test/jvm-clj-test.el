;;; jvm-clj-test.el --- Tests for built-in Lisp access from Clojure -*- lexical-binding: t; -*-

(require 'ert)
(require 'jvm)
(require 'jvm-clj)

(defvar jvm-clj-test--jars-dir (getenv "EMACS_JNI_CLOJURE_JARS_DIR")
  "Directory containing Clojure/nREPL jars, or nil if unset. See
`jvm-clj-eval-test--jars-dir' in test/jvm-clj-eval-test.el for why tests in
this file skip cleanly (not fail) when this isn't configured.")

(defvar jvm-clj-test--ready nil
  "Set to t once the JVM is started and `emacs-jni.clj' is loaded, so
every test in this file can call `jvm-clj-test--ensure-ready' idempotently
regardless of ERT's execution order.")

(defun jvm-clj-test--skip-unless-jars ()
  (unless (and jvm-clj-test--jars-dir (file-directory-p jvm-clj-test--jars-dir))
    (ert-skip "Set EMACS_JNI_CLOJURE_JARS_DIR (see test/fetch-clojure-jars.sh) to run this test")))

(defun jvm-clj-test--ensure-ready ()
  (jvm-clj-test--skip-unless-jars)
  (unless jvm-clj-test--ready
    (setq jvm-clj-classpath (directory-files jvm-clj-test--jars-dir t "\\.jar\\'"))
    (jvm-clj-start-await)
    (setq jvm-clj-test--ready t)))

(defun jvm-clj-test--eval (code)
  "Evaluate CODE (Clojure source) via `jvm-clj-eval-string-await' and return
its result value (signalling `jvm-error' on failure)."
  (jvm-clj-eval-string-await code))

(defun jvm-clj-test--prin1 (lisp-object)
  "Call `prin1ToString' on the wrapped Java LISP-OBJECT via plain
`jvm-call-await' reflection (the same generic path any other JVM-object
method call uses -- a Java `emacs_jni.core.LispObject' has no special status on
the Lisp side, it's just an opaque wrapped handle, per the design spec's
non-goal: \"marshalling arbitrary Lisp values to Clojure data\")."
  (jvm-call-await lisp-object "prin1ToString" nil))

(ert-deftest emacs-jni-jvm-clj-test/intern-returns-a-clojure-symbol ()
  (jvm-clj-test--ensure-ready)
  ;; intern now yields a Clojure symbol, not an opaque LispObject.
  (should (eq (jvm-clj-test--eval "(symbol? (emacs-jni.clj/intern \"car\"))") t))
  (should (equal (jvm-clj-test--eval "(str (emacs-jni.clj/intern \"car\"))") "car")))

(ert-deftest emacs-jni-jvm-clj-test/intern-colon-name-returns-a-clojure-keyword ()
  (jvm-clj-test--ensure-ready)
  ;; A leading ":" interns a Lisp keyword, which maps to a Clojure keyword.
  (should (eq (jvm-clj-test--eval "(keyword? (emacs-jni.clj/intern \":foo\"))") t))
  (should (equal (jvm-clj-test--eval "(name (emacs-jni.clj/intern \":foo\"))") "foo")))

(ert-deftest emacs-jni-jvm-clj-test/lisp-symbol-result-becomes-a-clojure-symbol ()
  (jvm-clj-test--ensure-ready)
  ;; Both directions at once: (symbol "intern") crosses Clojure->Lisp as
  ;; the Lisp function `intern', whose result (a Lisp symbol foo) crosses
  ;; back Lisp->Clojure as a Clojure symbol.
  (should (eq (jvm-clj-test--eval
               "(symbol? (emacs-jni.clj/apply (symbol \"intern\") \"foo\"))")
              t))
  (should (equal (jvm-clj-test--eval
                  "(str (emacs-jni.clj/apply (symbol \"intern\") \"foo\"))")
                 "foo")))

(ert-deftest emacs-jni-jvm-clj-test/clojure-symbol-arg-becomes-a-lisp-symbol ()
  (jvm-clj-test--ensure-ready)
  ;; (symbol "symbol-name") is the receiver, (symbol "car") the argument;
  ;; both cross as Lisp symbols. (symbol-name 'car) => "car".
  (should (equal (jvm-clj-test--eval
                  "(emacs-jni.clj/apply (symbol \"symbol-name\") (symbol \"car\"))")
                 "car")))

(ert-deftest emacs-jni-jvm-clj-test/clojure-keyword-arg-becomes-a-lisp-keyword ()
  (jvm-clj-test--ensure-ready)
  ;; A Lisp keyword's `symbol-name' includes the leading colon.
  (should (equal (jvm-clj-test--eval
                  "(emacs-jni.clj/apply (symbol \"symbol-name\") (keyword \"foo\"))")
                 ":foo")))

(ert-deftest emacs-jni-jvm-clj-test/namespaced-symbol-round-trips-by-full-name ()
  (jvm-clj-test--ensure-ready)
  (should (equal (jvm-clj-test--eval
                  "(str (emacs-jni.clj/apply (symbol \"intern\") \"foo/bar\"))")
                 "foo/bar")))

(ert-deftest emacs-jni-jvm-clj-test/interned-symbol-round-trips-eq ()
  (jvm-clj-test--ensure-ready)
  ;; apply returns the Lisp symbol foo (as a Clojure symbol); eq'ing it
  ;; with a fresh (symbol "foo") re-interns to the same Lisp symbol.
  (should (eq (jvm-clj-test--eval
               "(emacs-jni.clj/eq (emacs-jni.clj/apply (symbol \"intern\") \"foo\")
                                        (symbol \"foo\"))")
              t)))

(ert-deftest emacs-jni-jvm-clj-test/uninterned-lisp-symbol-stays-a-lisp-object ()
  (jvm-clj-test--ensure-ready)
  ;; make-symbol yields an uninterned symbol; it is NOT converted to a
  ;; Clojure symbol (identity preserved), so `symbol?' is false and the
  ;; value stays an opaque LispObject.
  (should (eq (jvm-clj-test--eval
               "(symbol? (emacs-jni.clj/apply (symbol \"make-symbol\") \"uninterned-xyz\"))")
              nil))
  (let ((result (jvm-clj-test--eval
                 "(emacs-jni.clj/apply (symbol \"make-symbol\") \"uninterned-xyz\")")))
    (should (equal (jvm-clj-test--prin1 result) "uninterned-xyz"))))

(ert-deftest emacs-jni-jvm-clj-test/apply-integer-result-becomes-a-clojure-value ()
  (jvm-clj-test--ensure-ready)
  ;; `+' returns an integer -> a Clojure Long, not an opaque LispObject.
  ;; `jvm-clj-eval-string-await' then marshals that Long back to Lisp as 3.
  (should (equal (jvm-clj-test--eval
                  "(emacs-jni.clj/apply (emacs-jni.clj/intern \"+\") 1 2)")
                 3)))

(ert-deftest emacs-jni-jvm-clj-test/apply-string-result-becomes-a-clojure-string ()
  (jvm-clj-test--ensure-ready)
  (should (equal (jvm-clj-test--eval
                  "(emacs-jni.clj/apply (emacs-jni.clj/intern \"concat\") \"a\" \"b\")")
                 "ab")))

(ert-deftest emacs-jni-jvm-clj-test/apply-float-result-becomes-a-clojure-double ()
  (jvm-clj-test--ensure-ready)
  (should (equal (jvm-clj-test--eval
                  "(emacs-jni.clj/apply (emacs-jni.clj/intern \"+\") 1.0 0.5)")
                 1.5)))

(ert-deftest emacs-jni-jvm-clj-test/apply-nil-result-becomes-clojure-nil ()
  (jvm-clj-test--ensure-ready)
  ;; (not 1) => nil, converted to Clojure nil, marshalled back as Lisp nil.
  (should (eq (jvm-clj-test--eval
               "(emacs-jni.clj/apply (emacs-jni.clj/intern \"not\") 1)")
              nil)))

(ert-deftest emacs-jni-jvm-clj-test/apply-t-result-becomes-clojure-true ()
  (jvm-clj-test--ensure-ready)
  ;; (not nil) => t, converted to Clojure true, marshalled back as Lisp t.
  (should (eq (jvm-clj-test--eval
               "(emacs-jni.clj/apply (emacs-jni.clj/intern \"not\") nil)")
              t)))

(ert-deftest emacs-jni-jvm-clj-test/apply-non-scalar-result-stays-a-lisp-object ()
  (jvm-clj-test--ensure-ready)
  ;; A list is not a scalar, so it stays an opaque LispObject; read it
  ;; back with prin1ToString.
  (let ((result (jvm-clj-test--eval
                 "(emacs-jni.clj/apply (emacs-jni.clj/intern \"list\") 1 2 3)")))
    (should (equal (jvm-clj-test--prin1 result) "(1 2 3)"))))

(ert-deftest jvm-clj-test-apply-error-becomes-lisp-signal ()
  "A Lisp `error' surfaces to Clojure as an ex-info tagged ::lisp-signal,
carrying the signal symbol and data."
  (jvm-clj-test--ensure-ready)
  (should
   (equal
    (jvm-clj-test--eval
     "(try (emacs-jni.clj/apply (emacs-jni.clj/intern \"error\") \"boom\")
        (catch clojure.lang.ExceptionInfo e
          (let [d (ex-data e)]
            (str (:type d) \"|\" (:symbol d)))))")
    ":emacs-jni.clj/lisp-signal|error")))

(ert-deftest jvm-clj-test-throw-becomes-lisp-throw ()
  "A Lisp `throw' surfaces to Clojure as an ex-info tagged ::lisp-throw,
carrying tag and value."
  (jvm-clj-test--ensure-ready)
  (should
   (equal
    (jvm-clj-test--eval
     "(let [f (emacs-jni.clj/intern \"eval\")
            form (emacs-jni.clj/apply (emacs-jni.clj/intern \"read\")
                                       \"(throw 'my-tag 42)\")]
        (try (emacs-jni.clj/apply f form)
          (catch clojure.lang.ExceptionInfo e
            (let [d (ex-data e)]
              (str (:type d) \"|\" (:tag d) \"|\" (:value d))))))")
    ":emacs-jni.clj/lisp-throw|my-tag|42")))

(ert-deftest emacs-jni-jvm-clj-test/type-of-returns-a-clojure-symbol ()
  (jvm-clj-test--ensure-ready)
  ;; type-of yields the Lisp type symbol as a Clojure symbol (e.g.
  ;; `string' for a string value).
  (should (eq (jvm-clj-test--eval "(symbol? (emacs-jni.clj/type-of \"hello\"))") t))
  (should (equal (jvm-clj-test--eval "(str (emacs-jni.clj/type-of \"hello\"))") "string")))

(ert-deftest emacs-jni-jvm-clj-test/eq-is-true-for-the-same-symbol ()
  (jvm-clj-test--ensure-ready)
  (should (eq (jvm-clj-test--eval
               "(emacs-jni.clj/eq (symbol \"car\") (symbol \"car\"))")
              t)))

(ert-deftest emacs-jni-jvm-clj-test/eq-is-false-for-different-symbols ()
  (jvm-clj-test--ensure-ready)
  (should (eq (jvm-clj-test--eval
               "(emacs-jni.clj/eq (symbol \"car\") (symbol \"cdr\"))")
              nil)))

(ert-deftest emacs-jni-jvm-clj-test/is-not-nil-is-true-for-a-non-nil-symbol ()
  (jvm-clj-test--ensure-ready)
  (should (eq (jvm-clj-test--eval
               "(emacs-jni.clj/is-not-nil? (symbol \"car\"))")
              t)))

(ert-deftest emacs-jni-jvm-clj-test/is-not-nil-is-false-for-nil ()
  (jvm-clj-test--ensure-ready)
  (should (eq (jvm-clj-test--eval
               "(emacs-jni.clj/is-not-nil? nil)")
              nil)))

(ert-deftest emacs-jni-jvm-clj-test/clojure-calls-lisp-back-inside-a-lisp-to-clojure-call ()
  "Reentrancy: this test's OWN call is Lisp -> Clojure (`jvm-clj-eval-
string', serviced by `jvm-call-await's `accept-process-output' loop). The
Clojure code below itself calls back into Lisp twice (`apply', then
`is-not-nil?' on the result) *during* that same outer call -- each inner
round trip sends its own `(lisp-request)' wakeup, serviced by the SAME
still-running outer `accept-process-output' loop, before the outer
`eval' itself returns. See design spec, \"Core constraint\": \"a
`clj/apply' issued from inside a `jvm-clj-eval-string-await' is serviced by
the outer `-sync' wrapper's own `accept-process-output' loop.\""
  (jvm-clj-test--ensure-ready)
  (should (eq (jvm-clj-test--eval
               "(let [sym (emacs-jni.clj/intern \"car\")]
                  (emacs-jni.clj/is-not-nil? sym))")
              t)))

(ert-deftest jvm-clj-test-require-symbol-is-callable ()
  "require-symbol returns an IFn that funcalls the Lisp symbol."
  (jvm-clj-test--ensure-ready)
  ;; `jvm-clj-test--eval' reads CODE via `clojure.java.api.Clojure/read',
  ;; which is `clojure.edn/read-string' -- edn has no `'' quote reader
  ;; macro, so `'sym' would misread as a symbol literally named "'sym".
  ;; Spell it out as `(quote sym)' instead; same semantics.
  ;; `=' is numeric-only in Elisp; use `equal' for the string result.
  (should
   (equal (jvm-clj-test--eval
           "((emacs-jni.clj/require-symbol (quote concat)) \"a\" \"b\")")
          "ab"))
  (should
   (= (jvm-clj-test--eval
       "(clojure.core/apply (emacs-jni.clj/require-symbol (quote +)) [1 2 3])")
      6)))

(ert-deftest jvm-clj-test-refer-symbols-defs-vars ()
  "refer-symbols creates callable vars in the current namespace."
  (jvm-clj-test--ensure-ready)
  (should
   (equal (jvm-clj-test--eval
           "(do (emacs-jni.clj/refer-symbols concat) (concat \"x\" \"y\"))")
          "xy")))

(ert-deftest jvm-clj-test-require-function-is-early-bound ()
  "require-function captures the function now; a later fset of the symbol
does not change the imported function, while require-symbol tracks it."
  (jvm-clj-test--ensure-ready)
  (should
   (equal
    (jvm-clj-test--eval
     "(do (emacs-jni.clj/apply (emacs-jni.clj/intern \"defalias\")
            (emacs-jni.clj/intern \"jvm-clj-test-fn\") (emacs-jni.clj/intern \"car\"))
          (let [early (emacs-jni.clj/require-function (quote jvm-clj-test-fn))
                late  (emacs-jni.clj/require-symbol (quote jvm-clj-test-fn))]
            (emacs-jni.clj/apply (emacs-jni.clj/intern \"defalias\")
              (emacs-jni.clj/intern \"jvm-clj-test-fn\") (emacs-jni.clj/intern \"cdr\"))
            (let [lst (emacs-jni.clj/apply (emacs-jni.clj/intern \"list\") 1 2 3)]
              (str (early lst) \"|\" (str (late lst))))))")
    ;; early still `car' -> 1; late now `cdr' -> opaque LispObject (2 3),
    ;; whose prin1 is "(2 3)". Returned as a string to avoid depending on
    ;; vector return marshaling in jvm-clj-eval-string-await.
    "1|(2 3)")))

(provide 'jvm-clj-test)
;;; jvm-clj-test.el ends here
