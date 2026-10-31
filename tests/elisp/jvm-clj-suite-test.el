;;; jvm-clj-suite-test.el --- Run the Clojure tests in tests/clojure/ through ERT -*- lexical-binding: t; -*-

;; Each Clojure test becomes one ERT test for each path.  See
;; `jvm-clj-test-runner-define-ert-tests'.

(require 'ert)
(require 'jvm-clj-test-runner
         (expand-file-name "../jvm-clj-test-runner"
                           (file-name-directory (or load-file-name buffer-file-name))))

(jvm-clj-test-runner-define-ert-tests)

(provide 'jvm-clj-suite-test)
;;; jvm-clj-suite-test.el ends here
