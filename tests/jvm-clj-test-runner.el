;;; jvm-clj-test-runner.el --- Run the Clojure tests in tests/clojure/ -*- lexical-binding: t; -*-

;;; Commentary:

;; Run the `clojure.test' namespaces in tests/clojure/ in the JVM of this
;; Emacs.  Two runners use the same test files:
;;
;; - `jvm-clj-test-runner-run-batch-and-exit': Clojure runs the suite and
;;   prints its own report.  This shows the tests as a Clojure user sees
;;   them.
;; - `jvm-clj-test-runner-define-ert-tests': one ERT test for each Clojure
;;   test and path.  tests/elisp/jvm-clj-suite-test.el uses it.
;;
;; Each test runs inside a Lisp-to-Clojure call, so every call from the test
;; into Lisp is a callback.  Both runners use `emacs-jni.test-runner', in
;; tests/clojure/emacs_jni/test_runner.clj.

;;; Code:

(require 'ert)
(require 'jvm-clj)

(defconst jvm-clj-test-runner--root
  (expand-file-name "clojure/" (file-name-directory (or load-file-name buffer-file-name)))
  "Directory of the Clojure tests.  It is also their classpath entry.")

(defconst jvm-clj-test-runner-paths '(await direct)
  "Paths that a Clojure test can run on.
`await': a JVM thread runs the test.  Its calls into Lisp go through the
queue, which the `-await' wait loop empties.
`direct': the Emacs main thread runs the test.  Its calls into Lisp run
at once.")

(defconst jvm-clj-test-runner--suite-timeout 600
  "Seconds that the whole suite can run on the `await' path.")

(defun jvm-clj-test-runner--start ()
  "Start Clojure, and load `emacs-jni.test-runner'."
  (jvm-clj-start-await)
  (jvm-load-jar-await jvm-clj-test-runner--root)
  (jvm-clj-require-await "emacs-jni.test-runner"))

(defun jvm-clj-test-runner--call (path function args &optional timeout)
  "Call `emacs-jni.test-runner/FUNCTION' with ARGS on PATH.
Return the result.  ARGS is a list of strings.  PATH is an element of
`jvm-clj-test-runner-paths'.  TIMEOUT applies only to `await'."
  ;; `prin1' escapes a string the same way that the Clojure reader reads it.
  (let ((code (format "(emacs-jni.test-runner/%s %s)"
                      function (mapconcat #'prin1-to-string args " "))))
    (pcase path
      ('await (jvm-clj-eval-string-await code timeout))
      ('direct (jvm-clj-eval-string-direct code))
      (_ (error "Unknown path: %S" path)))))

(defun jvm-clj-test-runner--run-var (path name)
  "Run the Clojure test NAME on PATH, as the body of an ERT test.
If the test fails, print its `clojure.test' report and fail."
  (let ((report (jvm-clj-test-runner--call path "run-var" (list name))))
    (when report
      ;; ERT prints the condition with escaped newlines, so print the
      ;; report itself.
      (message "%s" report)
      (ert-fail (car (split-string report "\n" t))))))

(defun jvm-clj-test-runner-define-ert-tests ()
  "Define one ERT test for each Clojure test and path.
A test named NS/NAME on PATH becomes `emacs-jni-clojure-test/NS/NAME/PATH'.
This starts Clojure, because it must load the test namespaces to find
the tests."
  (jvm-clj-test-runner--start)
  (dolist (name (split-string (jvm-clj-test-runner--call
                               'await "test-var-names" (list jvm-clj-test-runner--root))
                              "\n" t))
    (dolist (path jvm-clj-test-runner-paths)
      (let ((test-name (intern (format "emacs-jni-clojure-test/%s/%s" name path))))
        (ert-set-test test-name
                      (make-ert-test
                       :name test-name
                       :body (lambda () (jvm-clj-test-runner--run-var path name))))))))

(defun jvm-clj-test-runner-run-batch-and-exit ()
  "Run the Clojure tests on each path, print their report, and exit Emacs.
Use it with `-f' in batch mode.  The arguments after it are the
namespaces to run; if there are none, run every test namespace.  Exit
with status 0 if all tests pass on every path, otherwise 1."
  (let ((args (cons jvm-clj-test-runner--root command-line-args-left))
        (failures 0))
    (setq command-line-args-left nil)
    (jvm-clj-test-runner--start)
    (dolist (path jvm-clj-test-runner-paths)
      (message "\n=== Path: %s ===" path)
      (setq failures (+ failures (jvm-clj-test-runner--call
                                  path "run-all" args
                                  jvm-clj-test-runner--suite-timeout))))
    (kill-emacs (if (zerop failures) 0 1))))

(provide 'jvm-clj-test-runner)
;;; jvm-clj-test-runner.el ends here
