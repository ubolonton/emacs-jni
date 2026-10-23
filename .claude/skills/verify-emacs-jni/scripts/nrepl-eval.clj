;; Usage: bb nrepl-eval.clj PORT CODE
;;
;; Sends one nREPL `eval' of CODE to 127.0.0.1:PORT and prints each reply's :value, :out, :err,
;; :ex, and :status, one per line. Exits 1 if a reply has :ex or an "eval-error" status.

(require '[bencode.core :as bencode])
(import '(java.net Socket) '(java.io PushbackInputStream))

(defn- text [v] (if (bytes? v) (String. ^bytes v "UTF-8") v))

(let [[port code] *command-line-args*
      socket (Socket. "127.0.0.1" (Integer/parseInt port))
      out (.getOutputStream socket)
      in (PushbackInputStream. (.getInputStream socket))
      failed (atom false)]
  (.setSoTimeout socket 30000)
  (bencode/write-bencode out {"op" "eval" "code" code "id" "1"})
  (loop []
    (let [reply (bencode/read-bencode in)
          status (set (map text (get reply "status")))]
      (doseq [k ["value" "out" "err" "ex"]]
        (when-let [v (get reply k)]
          (println (str k ": " (text v)))))
      (when (or (get reply "ex") (status "eval-error"))
        (reset! failed true))
      (if (status "done")
        (println (str "status: " (pr-str (sort status))))
        (recur))))
  (.close socket)
  (System/exit (if @failed 1 0)))
