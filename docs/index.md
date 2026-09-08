# emacs-jni documentation

This documentation follows [Diátaxis](https://diataxis.fr/). Diátaxis
splits documentation into four forms. Each form answers a different
question. Pick the section that matches your question.

| Section | Question it answers | When to read it |
|---|---|---|
| [Tutorials](tutorials/) | "Can you teach me?" | You are new to this project. You want a guided first success. |
| [How-to guides](how-to-guides/) | "How do I do X?" | You know what you want. You need the steps. |
| [Reference](reference/) | "What are the exact facts?" | You need a function signature, a default, or a status value. |
| [Explanation](explanation/) | "Why does it work this way?" | You want the design reasoning, not the steps. |

## Tutorials

Guided lessons. Follow them in order. Each one ends with a working
result.

- [Your first JVM call](tutorials/01-your-first-jvm-call.md)
- [Evaluate Clojure and connect an nREPL client](tutorials/02-evaluate-clojure-and-connect-nrepl.md)

## How-to guides

Task-focused recipes. Use them once you know the basics.

- [Control the JVM lifecycle](how-to-guides/control-the-jvm-lifecycle.md)
- [Load a jar](how-to-guides/load-a-jar.md)
- [Call Java from Lisp](how-to-guides/call-java-from-lisp.md)
- [Pass a Lisp function to Java](how-to-guides/pass-a-lisp-function-to-java.md)
- [Handle errors and timeouts](how-to-guides/handle-errors-and-timeouts.md)
- [Evaluate Clojure code](how-to-guides/evaluate-clojure-code.md)
- [Run an nREPL server](how-to-guides/run-an-nrepl-server.md)
- [Access Emacs Lisp from Clojure](how-to-guides/access-lisp-from-clojure.md)

## Reference

Technical facts about the Lisp API and its configuration surface.

- [Lisp API](reference/lisp-api.md)
- [Configuration](reference/configuration.md)

## Explanation

Background and design reasoning.

- [Architecture](explanation/architecture.md)
- [Interop design](explanation/interop-design.md)
- [Limitations and trade-offs](explanation/limitations-and-tradeoffs.md)

## Outside this documentation

- [`README.md`](../README.md) — build instructions and prerequisites.
- [`docs/superpowers/`](superpowers/) — the specs and plans written
  during development. These are historical design records, not user
  documentation.
