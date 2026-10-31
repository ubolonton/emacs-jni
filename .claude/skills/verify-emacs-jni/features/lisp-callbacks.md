# Lisp callbacks

The user passes a Lisp function as a Java argument. Java gets a `LispCallable`, calls it, and
gets a `CompletableFuture` with the Lisp return value. A Lisp signal in the function reaches Java
as a `LispSignal`. On the direct path, a signal that unwinds out of the Java call returns to Lisp
as the original signal.

## Sub-features

- `callback-direct` runs the function at once, on the main thread, inside a direct call.
- `callback-await` runs the function through the queue, from a worker-side call thread.
- `callback-signal` turns a Lisp signal into a `LispSignal` in Java.

## How to get to it (user POV)

- Put a lambda or a non-symbol function in the `ARGS` of `jvm-call`, `jvm-call-await`, or
  `jvm-call-async`, where the Java parameter accepts a `LispCallable` or `Object`.

## Driving it with verify-emacs

Preconditions:

- Baseline preconditions from the index. `(jvm-start)` returned `ready`.
- `calc.jar` is loaded: `$V eval "(jvm-load-jar \"$R/target/test-fixtures/calc.jar\")"`.

- **Direct.** Run `$V eval '(jvm-call "Calc" "callWithArg" (list (lambda (n) (* n 10)) 7))'`.
  Prints `70`.
- **Await (queue path).** Run
  `$V eval '(jvm-call-await "Calc" "callWithArg" (list (lambda (n) (* n 10)) 7))'`. Prints `70`.
  The call thread waits while the `-await` loop drains the queue and runs the lambda.
- **Signal reaches Java.** Run
  `$V eval '(jvm-call "Calc" "describeCallbackError" (list (lambda (n) (error "boom %s" n)) 1))'`.
  Prints `"error"`, the symbol name of the `LispSignal`.
- **Side effect.** Run
  `$V eval '(progn (setq v--seen nil) (jvm-call-await "Calc" "callWithArg" (list (lambda (n) (setq v--seen n)) 5)) v--seen)'`.
  Prints `5`.
- **Proof.** The transcript holds the result of each call. The side-effect journey shows Lisp state
  that Java changed.

## Gotchas

- A symbol with a function definition becomes a `LispSymbol`, not a `LispCallable`. Pass a lambda
  or `(symbol-function (quote f))`.
- A direct call that waits on a Java thread that calls the function deadlocks until the Java
  timeout: the main thread cannot drain the queue during a direct call.
- The fixture waits 10 s for the future.
