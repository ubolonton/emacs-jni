# Evaluate Clojure and connect an nREPL client

In this tutorial, we run Clojure code inside an Emacs-embedded JVM.
Then we start a real nREPL server and connect to it from a separate
terminal. By the end, an external nREPL client will evaluate Clojure
code running inside your Emacs process.

This tutorial builds on
[Your first JVM call](01-your-first-jvm-call.md). It assumes you can
already build the module and start the JVM.

## Step 1: Fetch the Clojure and nREPL jars

These jars are not bundled with this repository. Fetch them once:

```sh
./test/fetch-clojure-jars.sh
```

This downloads four jars into `target/clojure-jars/`: `clojure-1.12.0.jar`,
`spec.alpha-0.5.238.jar`, `core.specs.alpha-0.4.74.jar`, and
`nrepl-1.7.0.jar`.

## Step 2: Start Emacs with the jar directory set

```sh
cargo build
EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" emacs -Q -L elisp
```

Setting `EMACS_JNI_CLOJURE_JARS_DIR` before launch lets the next step
find the jars by reading this environment variable back from Lisp.

## Step 3: Start the JVM and load the jars

In `*scratch*`, evaluate each form one at a time.

```elisp
(require 'jvm-clj)

(jvm-start-sync)
;; => ready

(dolist (jar (directory-files (getenv "EMACS_JNI_CLOJURE_JARS_DIR") t "\\.jar\\'"))
  (jvm-load-jar-sync jar))
```

The `dolist` loads all four jars, one at a time, into the JVM's shared
classloader.

## Step 4: Evaluate Clojure code

```elisp
(jvm-clj-eval-string "(+ 40 (* 2 1))")
;; => (t 42 nil)
```

The result is `(OK RESULT ERR)`, the same shape `jvm-call-sync`
returns. Clojure evaluated the expression and returned `42`.

## Step 5: Start an nREPL server

```elisp
(setq my-nrepl (jvm-clj-nrepl-start 7888))
;; => (t (7888 . #<user-ptr ...>) nil)
```

`7888` is the port. The result's `RESULT` part is
`(ACTUAL-PORT . SERVER)`. `ACTUAL-PORT` confirms the port the server
bound. `SERVER` is a wrapped JVM object you will need later, to stop
the server.

A real nREPL server is now listening on `localhost:7888`, inside your
Emacs process.

## Step 6: Connect from a separate process

Open a new terminal. Any bencode-speaking nREPL client works here
(`lein repl :connect localhost:7888`, an editor's nREPL client, and so
on). This tutorial uses `nc`, so no extra tooling is required:

```sh
code='(+ 100 23)'
len=${#code}
printf 'd2:op4:eval4:code%d:%s2:id1:1e' "$len" "$code" | nc 127.0.0.1 7888
```

The output contains `5:value3:123` and `6:statusl4:donee`. That is a
real bencode-encoded nREPL response. `123` is `(+ 100 23)`, evaluated
by the Clojure runtime running inside Emacs, and sent back over the
network to this separate terminal.

## Step 7: Stop the server

Back in Emacs:

```elisp
(jvm-clj-nrepl-stop (cdr (nth 1 my-nrepl)))
;; => (t nil nil)
```

`(cdr (nth 1 my-nrepl))` pulls `SERVER` out of the result you stored in
Step 5.

You have now evaluated Clojure code inside Emacs, and reached it from
an independent process over the real nREPL wire protocol.

## What's next

- [How to evaluate Clojure code](../how-to-guides/evaluate-clojure-code.md)
  for namespace handling and return-value limits.
- [How to run an nREPL server](../how-to-guides/run-an-nrepl-server.md)
  for port choice and security notes.
- [How to access Emacs Lisp from Clojure](../how-to-guides/access-lisp-from-clojure.md)
  to call Lisp functions and hold Lisp values from Clojure.
- [Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md)
  for why `*ns*` does not carry over between separate eval calls.
