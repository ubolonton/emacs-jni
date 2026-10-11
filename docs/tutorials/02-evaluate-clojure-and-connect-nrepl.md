# Evaluate Clojure and connect an nREPL client

In this tutorial, we run Clojure code in the JVM inside Emacs. Then we start an nREPL server and
connect to it from a different terminal. At the end, an external nREPL client evaluates Clojure
code in your Emacs process.

This tutorial continues from [Your first JVM call](01-your-first-jvm-call.md). It assumes that you
can build the module and start the JVM.

## Step 1: Build the module and start Emacs

Run these commands from the repository root:

```sh
mise run build
emacs -Q -l dev-paths.el
```

## Step 2: Start the JVM and load Clojure

In `*scratch*`, evaluate each form one at a time.

```elisp
(require 'jvm-clj)

(jvm-clj-start)
;; => nil
```

On the first call, `jvm-clj-start` downloads four jars into `~/.m2/repository`: Clojure 1.12.0,
spec.alpha 0.5.238, core.specs.alpha 0.4.74, and nREPL 1.7.0. This can take some seconds. Later
calls use these jars again.

`jvm-clj-start` also starts the JVM. Then it loads the four jars and the `emacs-jni.clj` namespace.
Check the status:

```elisp
(jvm-status)
;; => ready
```

## Step 3: Evaluate Clojure code

```elisp
(jvm-clj-eval-string "(+ 40 (* 2 1))")
;; => 42
```

Clojure evaluated the expression. `jvm-clj-eval-string` returned the result as a Lisp integer.

## Step 4: Start an nREPL server

```elisp
(setq my-nrepl (jvm-clj-nrepl-start 7888))
;; => (7888 . #<user-ptr ...>)
```

`7888` is the port. `jvm-clj-nrepl-start` returns `(ACTUAL-PORT . SERVER)`. `ACTUAL-PORT` is the
port that the server listens on. `SERVER` is a wrapped JVM object. We use it in Step 6 to stop
the server.

An nREPL server now listens on `localhost:7888`, inside your Emacs process.

## Step 5: Connect from a separate process

Open a new terminal. We use `nc` to send one nREPL message:

```sh
code='(+ 100 23)'
len=${#code}
printf 'd2:op4:eval4:code%d:%s2:id1:1e' "$len" "$code" | timeout 3 nc 127.0.0.1 7888
```

The output contains `5:value3:123` and `6:statusl4:donee`. This is a bencode-encoded nREPL
response. The Clojure runtime in Emacs evaluated `(+ 100 23)` and sent `123` to this terminal.
`nc` keeps the connection open, so `timeout 3` closes it after 3 seconds.

## Step 6: Stop the server

In Emacs:

```elisp
(jvm-clj-nrepl-stop (cdr my-nrepl))
;; => #<user-ptr ...>
```

`(cdr my-nrepl)` is the `SERVER` value from Step 4. `jvm-clj-nrepl-stop` returns the stopped
server.

We evaluated Clojure code inside Emacs. Then a different process evaluated code there over the
nREPL protocol.

## What's next

- [How to evaluate Clojure code](../how-to-guides/evaluate-clojure-code.md) for namespaces and
  result types.
- [How to run an nREPL server](../how-to-guides/run-an-nrepl-server.md) for ports, bind
  addresses, and security.
- [How to access Emacs Lisp from Clojure](../how-to-guides/access-lisp-from-clojure.md) to call
  Lisp functions and hold Lisp values from Clojure.
