(module-load "target/debug/libjni_dyn.dylib")

(ert-deftest loading ()
  (should (featurep 'jni-dyn)))

(ert-deftest abs ()
  (should (= (jni-abs -5) 5))
  (should (= (jni-abs 7) 7)))

(ert-deftest abs-unchecked ()
  (should (= (jni-abs-unchecked -5) 5))
  (should (= (jni-abs-unchecked 7) 7)))

(ert-deftest abs-elisp ()
  (should (= (abs -5) 5))
  (should (= (abs 7) 7)))

(ert-deftest sin ()
  (garbage-collect)
  (message "jni-sin %s" (benchmark-run 1 (jni-sin -1.1)))
  (garbage-collect)
  (message "jni-sin %s" (benchmark-run 1 (jni-sin -1.0)))
  (garbage-collect)
  (message "jni-sin %s" (benchmark-run 1000 (jni-sin -1.0)))
  (garbage-collect)
  (message "    sin %s" (benchmark-run 1 (sin -1.0)))
  (garbage-collect)
  (message "    sin %s" (benchmark-run 1000 (sin -1.0))))
