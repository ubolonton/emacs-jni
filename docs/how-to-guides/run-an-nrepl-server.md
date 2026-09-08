# How to run an nREPL server

This guide assumes the JVM is `ready` and the Clojure and nREPL jars
are already loaded. See
[Evaluate Clojure and connect an nREPL client](../tutorials/02-evaluate-clojure-and-connect-nrepl.md)
for the setup steps.

## Start a server on a specific port

```elisp
(setq my-nrepl (jvm-nrepl-start 7888))
;; => (t (7888 . #<user-ptr ...>) nil)
```

`RESULT` is `(ACTUAL-PORT . SERVER)`. Keep the whole `my-nrepl` value.
You need `SERVER` later, to stop the server.

## Let the OS pick a free port

```elisp
(setq my-nrepl (jvm-nrepl-start))
;; => (t (54321 . #<user-ptr ...>) nil)
```

Calling `jvm-nrepl-start` with no argument passes port `0`, which
means "any free port." Read the real port back from `RESULT`'s `car`:

```elisp
(car (nth 1 my-nrepl))
;; => 54321
```

## Bind to a routable interface

By default the server binds loopback (`127.0.0.1`) only — nREPL's own
default. A loopback-only server is unreachable through VM or container
port forwarding: the forward delivers the connection to a routable
interface (the guest's `eth0`, say), not to the guest's `127.0.0.1`, so
the connection is refused and the client sees an empty `clone` response
("Could not open new session").

Pass a bind address as the second argument to listen beyond loopback:

```elisp
(setq my-nrepl (jvm-nrepl-start 8000 "0.0.0.0"))
;; => (t (8000 . #<user-ptr ...>) nil)
```

`"0.0.0.0"` listens on every interface; a specific address narrows it.
`jvm-clj-nrepl-start` takes the same argument. Read
[Restrict who can connect](#restrict-who-can-connect) before binding
beyond loopback.

## Connect a client

Any bencode-speaking nREPL client works: `lein repl :connect
localhost:PORT`, a `clj`-based client, an editor's built-in nREPL
client, or a raw socket. See
[the tutorial's connection step](../tutorials/02-evaluate-clojure-and-connect-nrepl.md#step-6-connect-from-a-separate-process)
for a minimal `nc`-based example that needs no extra tooling.

## Stop the server

```elisp
(jvm-nrepl-stop (cdr (nth 1 my-nrepl)))
;; => (t nil nil)
```

`(cdr (nth 1 my-nrepl))` pulls `SERVER` out of the value
`jvm-nrepl-start` returned.

## Preload the Lisp-access namespace

If you want connected nREPL clients to reach Emacs Lisp through
`dev.ublt.emacs.clj`, start the server with `jvm-clj-nrepl-start`
(from `jvm-clj.el`) instead of `jvm-nrepl-start`. It is the same call,
but it loads that namespace first. Stop it with `jvm-clj-nrepl-stop`,
or `jvm-nrepl-stop` — they are equivalent. See
[How to access Emacs Lisp from Clojure](access-lisp-from-clojure.md).

## Restrict who can connect

`jvm-nrepl-start` uses nREPL's defaults throughout: loopback bind, no
authentication, no TLS, no middleware configuration. Anyone who can
reach the port can evaluate arbitrary code with the permissions of
your Emacs process. Binding beyond loopback (the `bind` argument above)
widens that reach to everyone who can reach that interface — do it only
on a network you trust, and prefer a specific routable address over
`"0.0.0.0"` when you can. This is out of scope for this module — see
[Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md).
If you need authentication or TLS, configure them through nREPL's own
middleware, by starting the server with `jvm-call`/`jvm-new` directly
against `nrepl.server/start-server` instead of through
`jvm-nrepl-start`.
