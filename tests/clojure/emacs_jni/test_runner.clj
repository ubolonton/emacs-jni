(ns emacs-jni.test-runner
  "Finds and runs the `clojure.test` namespaces under a test root directory.

  Two runners use the same test files:

  - [[run-all]] backs the Clojure-owned run. It prints the standard
    `clojure.test` report.
  - [[test-var-names]] and [[run-var]] back the ERT bridge, which makes one
    ERT test for each test var.

  The results are strings and numbers, so that they cross to Lisp as Lisp
  values. See `tests/jvm-clj-test-runner.el`."
  (:require [clojure.java.io :as io]
            [clojure.string :as str]
            [clojure.test :as t])
  (:import [java.io File StringWriter]))

(defn- file->ns-name
  "Returns the namespace symbol of the Clojure source `file` under `root`."
  [root ^File file]
  (let [relative (.relativize (.toPath (io/file root)) (.toPath file))]
    (-> (str/join "." (map str relative))
        (str/replace #"\.clj$" "")
        (str/replace "_" "-")
        symbol)))

(defn- find-namespaces
  "Returns the sorted symbols of the `*_test.clj` namespaces under `root`."
  [root]
  (->> (file-seq (io/file root))
       (filter #(str/ends-with? (.getName ^File %) "_test.clj"))
       (map #(file->ns-name root %))
       sort))

(defn- load-namespaces
  "Requires the namespaces named by the strings `ns-names`, or every test
  namespace under `root` if `ns-names` is empty. Returns their symbols."
  [root ns-names]
  (let [namespaces (if (seq ns-names) (map symbol ns-names) (find-namespaces root))]
    (clojure.core/apply require namespaces)
    namespaces))

(defn run-all
  "Runs the test namespaces `ns-names`, or every one under `root`, and prints
  the report. Returns the count of failures and errors."
  [root & ns-names]
  (let [{:keys [fail error]} (clojure.core/apply t/run-tests (load-namespaces root ns-names))]
    (flush)
    (+ fail error)))

(defn test-var-names
  "Returns the qualified names of the test vars under `root`, one per line.

  The order is by namespace, then by definition."
  [root]
  (->> (load-namespaces root nil)
       (mapcat (fn [namespace]
                 (->> (vals (ns-interns namespace))
                      (filter (comp :test meta))
                      (sort-by (comp :line meta)))))
       (map #(str (symbol %)))
       (str/join "\n")))

(defn run-var
  "Runs the test var named `qualified-name`. Returns nil if it passed, else
  its report.

  `clojure.test/test-vars` runs the namespace's fixtures, but it runs the
  `:once` fixtures for each var, not once for the namespace."
  [qualified-name]
  (let [test-var (requiring-resolve (symbol qualified-name))
        out (StringWriter.)]
    ;; `test-vars` counts results only if a caller binds the counters, as `run-tests` does.
    (binding [t/*test-out* out
              t/*report-counters* (ref t/*initial-report-counters*)]
      (t/test-vars [test-var])
      (let [{:keys [fail error]} @t/*report-counters*]
        (when (pos? (+ fail error))
          (str out))))))
