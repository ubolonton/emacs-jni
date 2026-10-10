# How to run an nREPL server

This guide assumes that `jvm-clj-classpath` contains the Clojure and nREPL jars. See
[Start the Clojure side](access-lisp-from-clojure.md#start-the-clojure-side).
`jvm-clj-nrepl-start` calls `jvm-clj-start` for you.

## Start a server on a specific port

```elisp
(setq my-nrepl (jvm-clj-nrepl-start 7888))
;; => (7888 . #<user-ptr ...>)
```

`jvm-clj-nrepl-start` returns `(ACTUAL-PORT . SERVER)`. It signals `jvm-error` on failure. Keep
the value. You need `SERVER` to stop the server.

## Let the OS select a free port

```elisp
(setq my-nrepl (jvm-clj-nrepl-start))
;; => (54321 . #<user-ptr ...>)

(car my-nrepl)
;; => 54321
```

Without a port argument, the server uses port `0`. The OS then selects a free port.

## Bind to a routable interface

The server binds only to loopback (`127.0.0.1`) by default. VM or container port forwarding cannot
reach a loopback-only server. To listen on other interfaces, give a bind address as the second
argument:

```elisp
(setq my-nrepl (jvm-clj-nrepl-start 8000 "0.0.0.0"))
;; => (8000 . #<user-ptr ...>)
```

`"0.0.0.0"` listens on all interfaces. A specific address listens on that interface only. Read
[Restrict who can connect](#restrict-who-can-connect) first.

## Connect a client

Use any nREPL client, for example `lein repl :connect localhost:PORT` or the nREPL client of your
editor. For a minimal `nc` example, see
[the tutorial](../tutorials/02-evaluate-clojure-and-connect-nrepl.md#step-6-connect-from-a-separate-process).

## Stop the server

```elisp
(jvm-clj-nrepl-stop (cdr my-nrepl))
;; => #<user-ptr ...>
```

`(cdr my-nrepl)` is the `SERVER` value from `jvm-clj-nrepl-start`.

## Set a timeout

Use `jvm-clj-nrepl-start-await` and `jvm-clj-nrepl-stop-await`. These run on the worker thread.
Their last argument is the timeout in seconds:

```elisp
(setq my-nrepl (jvm-clj-nrepl-start-await 7888 nil 60))
(jvm-clj-nrepl-stop-await (cdr my-nrepl) 60)
```

## Use Lisp from an nREPL client

`jvm-clj-nrepl-start` loads `emacs-jni.clj` before it starts the server. A client can use it
immediately. See [How to access Emacs Lisp from Clojure](access-lisp-from-clojure.md).

## Restrict who can connect

The server has no authentication and no TLS. A client that can reach the port can run any code
with the permissions of your Emacs process.

- Keep the default loopback bind if you can.
- Bind to other interfaces only on a network that you trust.
- Use a specific address, not `"0.0.0.0"`, if you can.

For other server options, such as TLS, call `nrepl.server/start-server` from Clojure, not
`jvm-clj-nrepl-start`. See the nREPL documentation, and
[Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md).
