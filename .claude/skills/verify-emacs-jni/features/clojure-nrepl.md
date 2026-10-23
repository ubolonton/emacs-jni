# Clojure and nREPL

The user starts Clojure in the embedded JVM, evaluates Clojure source from Lisp, and serves an
nREPL client. Clojure code reads and changes Lisp state through `emacs-jni.clj`. From an nREPL
thread, each Lisp request goes through the queue and runs on the Emacs main thread when Emacs is
idle.

## Sub-features

- `clj-start` loads the Clojure and nREPL jars and the `emacs-jni.clj` namespace.
- `clj-eval` evaluates Clojure source and converts the last value to Lisp.
- `clj-lisp-direct` calls Lisp from Clojure inside a direct eval.
- `nrepl-start` starts an nREPL server and returns `(PORT . SERVER)`.
- `nrepl-eval` evaluates code from an external nREPL client.
- `nrepl-lisp` changes Lisp state from an nREPL session (queue path).
- `nrepl-error` reports a Clojure exception to the client.
- `nrepl-stop` stops the server.

## How to get to it (user POV)

- `(jvm-clj-start)`, `(jvm-clj-eval-string CODE)`, `(jvm-clj-nrepl-start)`,
  `(jvm-clj-nrepl-stop SERVER)`, and their `-await` forms.
- Any nREPL client, connected to `127.0.0.1:PORT`.

## Driving it with verify-emacs

Preconditions:

- Baseline preconditions from the index. `(jvm-start)` returned `ready`, or the JVM is
  `not-started` (`jvm-clj-start` starts it).
- The Clojure jars are in `~/.m2/repository`, or the network can reach Maven Central.

- **Start Clojure.** Run `$V eval '(jvm-clj-start)'`. Exits 0. The first run can take minutes to
  download the jars.
- **Eval.** Run `$V eval '(jvm-clj-eval-string "(+ 40 (* 2 1))")'`. Prints `42`.
- **Lisp from Clojure, direct.** Run
  `$V eval '(jvm-clj-eval-string "(do (require (quote [emacs-jni.clj :as l])) (l/apply (quote +) 1 2))")'`.
  Prints `3`.
- **Start nREPL.** Run `$V eval '(car (setq v--nrepl (jvm-clj-nrepl-start)))'`. Prints the port.
  Store it: `P=$($V eval '(car v--nrepl)')`.
- **nREPL eval.** Run `$V nrepl $P '(+ 1 2)'`. Prints `value: 3` and `status: ("done")`.
- **Lisp state from nREPL.** Run `$V eval '(defvar v--from-nrepl nil)'`, then
  `$V nrepl $P '(require (quote [emacs-jni.clj :as l])) (l/apply (quote set) (quote v--from-nrepl) "hello from nREPL")'`.
  Prints `value: "hello from nREPL"`. Then `$V eval 'v--from-nrepl'` prints `"hello from nREPL"`.
- **nREPL error.** Run `$V nrepl $P '(/ 1 0)'`. Exit 1, with `err:` that contains
  `Divide by zero` and `ex: class java.lang.ArithmeticException`.
- **Stop nREPL.** Run `$V eval '(progn (jvm-clj-nrepl-stop (cdr v--nrepl)) t)'`. Prints `t`. Then
  `$V nrepl $P '(+ 1 2)'` exits 1 with `Connection refused`.
- **Proof.** The transcript holds the nREPL replies and the `eval` that reads back
  `v--from-nrepl`.

## Gotchas

- `jvm-clj-eval-string` resets `*ns*` after each call. Require namespaces in the same string.
- The server binds to loopback. The `nrepl` helper connects to `127.0.0.1` only.
- A Lisp request from nREPL waits until the daemon is idle. While an `eval` runs, the request
  waits; `l/apply` has no timeout.
- `(jvm-clj-nrepl-start)` with no port lets the OS choose one. Do not hard-code a port.
