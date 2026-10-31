# Call Java

The user calls a public Java method or constructor from Lisp and gets a Lisp value back. A class
name string is the target of a static call or a constructor. A wrapped JVM object is the target of
an instance call. Failures signal `jvm-error` (Java side) or `jvm-invalid-argument` (a Lisp value
with no marshalling rule).

## Sub-features

- `call-static` calls a static method and converts the result.
- `call-new` creates an object with `jvm-new` and returns a wrapped JVM object.
- `call-instance` calls a method on a wrapped JVM object.
- `call-await` runs a call on a worker-side thread and waits.
- `call-async` runs a call on a worker-side thread and delivers the result to a callback.
- `call-error` reports a missing class, a missing member, or a Java exception as `jvm-error`.
- `call-invalid-argument` rejects an unsupported Lisp argument before the call.

## How to get to it (user POV)

- `(jvm-call TARGET MEMBER ARGS)` and `(jvm-new CLASS ARGS)`: the direct path.
- `jvm-call-await`, `jvm-new-await`: the worker path, blocking with a timeout.
- `jvm-call-async`, `jvm-new-async`: the worker path, with a callback `(ok result err)`.

## Driving it with verify-emacs

Preconditions:

- Baseline preconditions from the index. `(jvm-start)` returned `ready`.

- **Static, direct.** Run `$V eval '(jvm-call "java.lang.Integer" "toHexString" (list 255))'`.
  Prints `"ff"`.
- **Static, await.** Run `$V eval '(jvm-call-await "java.lang.System" "getProperty" (list "os.name"))'`.
  Prints the OS name, for example `"Linux"`.
- **Constructor and instance.** Run
  `$V eval '(let ((list (jvm-new "java.util.ArrayList" nil))) (jvm-call list "add" (list "a")) (jvm-call list "size" nil))'`.
  Prints `1`.
- **Async.** Run
  `$V eval '(progn (setq v--async nil) (jvm-call-async "java.lang.Math" "max" (list 3 9) (lambda (ok r err) (setq v--async (list ok r err)))) nil)'`.
  Prints `nil`. Then poll `$V eval 'v--async'` until it prints `(t 9 nil)`.
- **Async error.** Run
  `$V eval '(progn (setq v--async nil) (jvm-call-async "No.Such" "x" nil (lambda (ok r err) (setq v--async (list ok r err)))) nil)'`,
  then poll `$V eval '(car v--async)'`. When `v--async` is set, `(car v--async)` is `nil` and
  `(nth 2 v--async)` is a string that contains `No.Such`.
- **Missing class.** Run `$V eval '(jvm-call "No.Such" "x" nil)'`. Exit 1 with
  `*ERROR*: JVM call failed:` and a message that names the class (see Gotchas).
- **Invalid argument.** Run `$V eval '(jvm-call "java.lang.Math" "abs" (list [1]))'`. Exit 1 with
  `*ERROR*: Unsupported Lisp value for a JVM call argument: [1]`.
- **Proof.** The transcript holds each call and its value. For an async journey, it holds the
  `nil` return and the polled callback value.

## Gotchas

- Overload resolution takes the first candidate that accepts the arguments, not the most specific
  one. `(jvm-call "java.lang.String" "valueOf" (list 42))` returns `"42.0"`.
- `nil` becomes Java `null` (or `false` for a boolean parameter). Lisp vectors, conses, and hash
  tables signal `jvm-invalid-argument`.
- An async callback runs only when the daemon drains the queue. Poll with more `eval` calls; one
  `eval` that sleeps blocks the drain.
- The missing-class message depends on the state. Before any jar load it is `failed to resolve
  Java class 'No/Such' (class not found or linkage error)`, with `/`. After a jar load it is
  `Caught Java exception: No.Such` with a stack trace from `URLClassLoader`.
- A Rust panic in an `-await` call thread shows only as `jvm-timeout` after 30 s.
