# Your first JVM call

In this tutorial, we start a JVM inside Emacs. We call a Java method from Lisp. Then we let Java
call a Lisp function. At the end, Emacs and the JVM call each other in both directions.

This tutorial uses a checkout of this repository. Install the tools in the
[Prerequisites](../../README.md#prerequisites) section of the README first.

## Step 1: Build the module and the test jars

Run this command from the repository root:

```sh
mise run fixtures
```

It builds the module in `target/debug/`. Then it builds two test jars in
`target/test-fixtures/`: `greeter.jar` and `calc.jar`. `tests/fixtures/Greeter.java` is a small
class:

```java
public class Greeter {
    public static String greet() {
        return "hello from Greeter";
    }
}
```

We use `calc.jar` in Step 5.

## Step 2: Start Emacs with the checkout on the load path

```sh
emacs -Q -l dev-paths.el
```

`-Q` skips your personal init file. `dev-paths.el` puts `elisp/` on the Emacs `load-path`. It
also tells `jvm.el` to load the module from `target/debug/`.

## Step 3: Load the library and start the JVM

Open `*scratch*` or `M-x ielm`. Evaluate each form one at a time.

```elisp
(require 'jvm)

(jvm-start)
;; => ready
```

`jvm-start` blocks until the JVM is ready. Check the status:

```elisp
(jvm-status)
;; => ready
```

If you see `start-failed`, read the `*jvm-log*` buffer for the reason:

```elisp
(with-current-buffer "*jvm-log*" (buffer-string))
```

## Step 4: Load the jar and call a static method

```elisp
(jvm-load-jar (expand-file-name "target/test-fixtures/greeter.jar"))
;; => nil
```

`jvm-load-jar` returns `nil` on success. It signals `jvm-error` on failure.

Now call `Greeter.greet()`:

```elisp
(jvm-call "Greeter" "greet" nil)
;; => "hello from Greeter"
```

`jvm-call` returns the value that Java returned. It signals `jvm-error` on failure.

We called a static Java method from Lisp.

## Step 5: Load a second jar

`Calc` has a constructor, an instance method, and the method `callWithArg`, which takes a Lisp
function as a parameter. We use `callWithArg` in Step 7.

Load the jar:

```elisp
(jvm-load-jar (expand-file-name "target/test-fixtures/calc.jar"))
;; => nil
```

## Step 6: Construct an object and call an instance method

```elisp
(jvm-new "Calc" (list 100))
;; => #<user-ptr ...>
```

`jvm-new` constructs a `Calc` object with `base` set to 100. It returns a wrapped JVM object,
which prints as `#<user-ptr ...>`. Store it, and call a method on it:

```elisp
(setq my-calc (jvm-new "Calc" (list 100)))

(jvm-call my-calc "addToBase" (list 23))
;; => 123
```

We passed the wrapped object as `TARGET`, so `jvm-call` made an instance call.

## Step 7: Let Java call a Lisp function

```elisp
(jvm-call "Calc" "callWithArg" (list (lambda (n) (* n 10)) 4))
;; => 40
```

The first argument is a Lisp `lambda`. It goes into the JVM as a callable Java object.
`Calc.callWithArg` calls it with `4` and waits for the result. The Lisp function multiplied `4`
by 10, and Java returned `40` to Lisp.

We called into the JVM, and the JVM called back into Lisp.

## What's next

- [How to call Java from Lisp](../how-to-guides/call-java-from-lisp.md) for all argument types.
- [How to pass a Lisp function to Java](../how-to-guides/pass-a-lisp-function-to-java.md) for
  named functions and errors in callbacks.
- [How to handle errors and timeouts](../how-to-guides/handle-errors-and-timeouts.md) for failed
  calls.
- [Interop design](../explanation/interop-design.md) for how calls work.
