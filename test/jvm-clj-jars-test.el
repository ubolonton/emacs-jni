;;; jvm-clj-jars-test.el --- Tests for jvm-clj-ensure-jars -*- lexical-binding: t; -*-

;; Each test serves a fake artifact from a `file://' repository and uses a
;; temporary local repository, so no network access occurs.

(require 'ert)
(require 'jvm-clj)

(defconst jvm-clj-jars-test--content "fake jar content"
  "Content of the fake artifact.")

(defconst jvm-clj-jars-test--sha256 (secure-hash 'sha256 jvm-clj-jars-test--content)
  "The correct sha256 of the fake artifact.")

(defconst jvm-clj-jars-test--jar "example/demo/1.0/demo-1.0.jar"
  "Path of the fake artifact, relative to a repository.")

(defmacro jvm-clj-jars-test--with-repos (sha256 &rest body)
  "Run BODY with a fake remote repository and an empty local repository.
The remote holds one artifact, example:demo:1.0.  `jvm-clj-dependencies'
lists it with SHA256.  BODY can use `remote' and `local'."
  (declare (indent 1))
  `(let* ((remote (make-temp-file "jvm-clj-remote-" t))
          (local (make-temp-file "jvm-clj-local-" t))
          (jvm-clj-maven-local-repo local)
          (jvm-clj-dependencies
           (list (list :group "example" :artifact "demo" :version "1.0"
                       ;; file:///c:/foo on Windows.  file://c:/foo would make
                       ;; "c:" the host, which `url-file' fetches through FTP.
                       :repository (concat "file://"
                                           (unless (string-prefix-p "/" remote) "/")
                                           remote)
                       :sha256 ,sha256))))
     (unwind-protect
         (let ((jar (expand-file-name jvm-clj-jars-test--jar remote)))
           (make-directory (file-name-directory jar) t)
           (with-temp-file jar
             (insert jvm-clj-jars-test--content))
           ,@body)
       (delete-directory remote t)
       (delete-directory local t))))

(defun jvm-clj-jars-test--should-reject-checksum ()
  "Assert that `jvm-clj-ensure-jars' signals a sha256 mismatch."
  (let ((err (should-error (jvm-clj-ensure-jars))))
    (should (string-match-p "sha256" (cadr err)))))

(ert-deftest emacs-jni-jvm-clj-jars-test/downloads-into-maven-layout ()
  (jvm-clj-jars-test--with-repos jvm-clj-jars-test--sha256
    (let ((expected (expand-file-name jvm-clj-jars-test--jar local)))
      (should (equal (jvm-clj-ensure-jars) (list expected)))
      (should (file-exists-p expected)))))

(ert-deftest emacs-jni-jvm-clj-jars-test/reuses-cached-jar ()
  (jvm-clj-jars-test--with-repos jvm-clj-jars-test--sha256
    (jvm-clj-ensure-jars)
    (delete-directory (expand-file-name "example" remote) t)
    (should (= (length (jvm-clj-ensure-jars)) 1))))

(ert-deftest emacs-jni-jvm-clj-jars-test/rejects-checksum-mismatch ()
  (jvm-clj-jars-test--with-repos (make-string 64 ?0)
    (jvm-clj-jars-test--should-reject-checksum)
    (should-not (directory-files-recursively local "jar"))))

(ert-deftest emacs-jni-jvm-clj-jars-test/rejects-corrupt-cached-jar ()
  (jvm-clj-jars-test--with-repos jvm-clj-jars-test--sha256
    (let ((cached (expand-file-name jvm-clj-jars-test--jar local)))
      (make-directory (file-name-directory cached) t)
      (with-temp-file cached (insert "corrupt"))
      (jvm-clj-jars-test--should-reject-checksum))))

(provide 'jvm-clj-jars-test)
;;; jvm-clj-jars-test.el ends here
