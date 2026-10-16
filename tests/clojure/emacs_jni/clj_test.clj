(ns emacs-jni.clj-test
  "Tests for access to Emacs Lisp from Clojure, through `emacs-jni.clj`."
  (:require [clojure.test :refer [deftest is]]
            [emacs-jni.clj :as l])
  (:import [emacs_jni.core LispObject LispSignal LispThrow]))

(l/refer-symbols string-to-number)

(deftest intern-returns-a-clojure-symbol
  (is (= 'car (l/intern "car"))))

(deftest intern-colon-name-returns-a-clojure-keyword
  ;; A leading ":" interns a Lisp keyword, which maps to a Clojure keyword.
  (is (= :foo (l/intern ":foo"))))

(deftest lisp-symbol-result-becomes-a-clojure-symbol
  ;; Both directions at once: 'intern crosses as the Lisp function `intern`, whose result (the
  ;; Lisp symbol foo) crosses back as a Clojure symbol.
  (is (= 'foo (l/apply 'intern "foo"))))

(deftest clojure-symbol-arg-becomes-a-lisp-symbol
  (is (= "car" (l/apply 'symbol-name 'car))))

(deftest clojure-keyword-arg-becomes-a-lisp-keyword
  ;; A Lisp keyword's `symbol-name` includes the leading colon.
  (is (= ":foo" (l/apply 'symbol-name :foo))))

(deftest namespaced-symbol-round-trips-by-full-name
  (is (= "foo/bar" (str (l/apply 'intern "foo/bar")))))

(deftest interned-symbol-round-trips-eq
  ;; A fresh 'foo interns to the same Lisp symbol that `intern` returned.
  (is (true? (l/eq (l/apply 'intern "foo") 'foo))))

(deftest uninterned-lisp-symbol-stays-a-lisp-object
  ;; An uninterned symbol keeps its identity only as a `LispObject`.
  (let [result (l/apply 'make-symbol "uninterned-xyz")]
    (is (instance? LispObject result))
    (is (= "uninterned-xyz" (str result)))))

(deftest apply-integer-result-becomes-a-clojure-long
  (is (= 3 (l/apply '+ 1 2)))
  (is (instance? Long (l/apply '+ 1 2))))

(deftest apply-string-result-becomes-a-clojure-string
  (is (= "ab" (l/apply 'concat "a" "b"))))

(deftest apply-float-result-becomes-a-clojure-double
  (is (= 1.5 (l/apply '+ 1.0 0.5))))

(deftest apply-nil-result-becomes-clojure-nil
  (is (nil? (l/apply 'not 1))))

(deftest apply-t-result-becomes-clojure-true
  (is (true? (l/apply 'not nil))))

(deftest apply-non-scalar-result-stays-a-lisp-object
  (let [result (l/apply 'list 1 2 3)]
    (is (instance? LispObject result))
    (is (= "(1 2 3)" (str result)))))

(deftest lisp-error-is-thrown-as-lisp-signal
  (let [^LispSignal e (is (thrown? LispSignal (l/apply 'error "boom")))]
    (is (= 'error (l/->clj (.-symbol e))))))

(deftest lisp-throw-is-thrown-as-lisp-throw
  (let [form (l/apply 'read "(throw 'my-tag 42)")
        ^LispThrow e (is (thrown? LispThrow (l/apply 'eval form)))]
    (is (= 'my-tag (l/->clj (.-tag e))))
    (is (= 42 (.-value e)))))

(deftest type-of-returns-a-clojure-symbol
  (is (= 'string (l/type-of "hello"))))

(deftest eq-is-true-for-the-same-symbol
  (is (true? (l/eq 'car 'car))))

(deftest eq-is-false-for-different-symbols
  (is (false? (l/eq 'car 'cdr))))

(deftest is-not-nil-is-true-for-a-non-nil-symbol
  (is (true? (l/is-not-nil? 'car))))

(deftest is-not-nil-is-false-for-nil
  (is (false? (l/is-not-nil? nil))))

(deftest require-symbol-is-callable
  (is (= "ab" ((l/require-symbol 'concat) "a" "b")))
  (is (= 6 (apply (l/require-symbol '+) [1 2 3]))))

(deftest refer-symbols-defines-callable-vars
  (is (= 42 (string-to-number "42"))))

(deftest require-function-is-early-bound
  ;; `early` keeps the function that the symbol had, and `late` calls the current one.
  (l/apply 'defalias 'emacs-jni-clj-test-fn 'car)
  (let [early (l/require-function 'emacs-jni-clj-test-fn)
        late (l/require-symbol 'emacs-jni-clj-test-fn)
        lst (l/apply 'list 1 2 3)]
    (l/apply 'defalias 'emacs-jni-clj-test-fn 'cdr)
    (is (= 1 (early lst)))
    ;; `cdr` returns a list, which stays a `LispObject`.
    (is (= "(2 3)" (str (late lst))))))
