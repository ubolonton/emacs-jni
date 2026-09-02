# Your first JVM call

In this tutorial, we start a JVM inside Emacs. We call a Java method
from Lisp. Then we let Java call a Lisp function back. By the end, you
will have a JVM and Emacs talking to each other in both directions.

This tutorial assumes a working build. If `cargo build` is new to you,
read the [Prerequisites](../../README.md#prerequisites) section of the
README first.

## Step 1: Build the module and a test jar

Run these commands from the repository root:

```sh
cargo build
mkdir -p target/test-fixtures
javac -d target/test-fixtures test/fixtures/Greeter.java
jar cf target/test-fixtures/greeter.jar -C target/test-fixtures Greeter.class
```

`test/fixtures/Greeter.java` is a small class already in this repository:

```java
public class Greeter {
    public static String greet() {
        return "hello from Greeter";
    }
}
```

`cargo build` produces `target/debug/libemacs_jni.so`. This is the
dynamic module that `elisp/emacs-jni.el` loads.

## Step 2: Start Emacs with the module on the load path

```sh
emacs -Q -L elisp
```

`-Q` skips your personal init file. `-L elisp` puts
`elisp/emacs-jni.el` on Emacs's `load-path`.

## Step 3: Load the library and start the JVM

Open `*scratch*` (or `M-x ielm`). Evaluate each form one at a time.

```elisp
(require 'emacs-jni)

(jvm-start-sync)
```

This blocks until the JVM is ready. On a normal machine, it takes
under a second. The result is the symbol `ready`.

Check the status:

```elisp
(jvm-status)
;; => ready
```

If you see `start-failed` instead, read the `*jvm-log*` buffer for the
reason:

```elisp
(with-current-buffer "*jvm-log*" (buffer-string))
```

## Step 4: Load the jar and call a static method

```elisp
(jvm-load-jar-sync (expand-file-name "target/test-fixtures/greeter.jar"))
;; => (t)
```

The result `(t)` is a cons `(OK . ERR)` with a `nil` `ERR`. Emacs
prints a `nil` cdr as nothing, so success prints as `(t)`.

Now call `Greeter.greet()`:

```elisp
(jvm-call-sync "Greeter" "greet" nil)
;; => (t "hello from Greeter" nil)
```

The result is a 3-element list: `(OK RESULT ERR)`. `OK` is `t`, so the
call succeeded. `RESULT` is the string Java returned.

You just called a static Java method from Lisp.

## Step 5: Build a second fixture with a constructor and a callback method

```sh
javac -cp target/emacs-jni-shim-classes -d target/test-fixtures test/fixtures/Calc.java
jar cf target/test-fixtures/calc.jar -C target/test-fixtures Calc.class
```

`target/emacs-jni-shim-classes` holds classes that `cargo build`
generates. `Calc.java` needs them on its classpath because one of its
methods (`callWithArg`, used in Step 7) takes a Lisp callback as a
parameter.

Load the new jar:

```elisp
(jvm-load-jar-sync (expand-file-name "target/test-fixtures/calc.jar"))
;; => (t)
```

## Step 6: Construct an object and call an instance method

```elisp
(jvm-new-sync "Calc" (list 100))
;; => (t #<user-ptr ...> nil)
```

`jvm-new-sync` constructs a `Calc` object with `base` set to 100. The
result is a wrapped JVM object, printed as `#<user-ptr ...>`. Store it:

```elisp
(setq my-calc (jvm-new-sync "Calc" (list 100)))

(jvm-call-sync (nth 1 my-calc) "addToBase" (list 23))
;; => (t 123 nil)
```

`(nth 1 my-calc)` pulls the wrapped object out of the `(OK RESULT ERR)`
list. Passing it as `TARGET` makes this an instance call, not a static
one.

## Step 7: Let Java call a Lisp function back

```elisp
(jvm-call-sync "Calc" "callWithArg" (list (lambda (n) (* n 10)) 4))
;; => (t 40 nil)
```

Notice the first argument: a real Lisp `lambda`. It crosses into the
JVM automatically, wrapped as a callable Java object. `Calc.callWithArg`
calls it from Java with `4`, waits for the result, and returns it. The
Lisp function ran, doubled back through Java, and landed here as `40`.

You have now called into the JVM, and had the JVM call back into Lisp.

## What's next

This tutorial stayed narrowly on the happy path. For real work, see:

- [How to call Java from Lisp](../how-to-guides/call-java-from-lisp.md)
  for the full range of argument types.
- [How to pass a Lisp function to Java](../how-to-guides/pass-a-lisp-function-to-java.md)
  for callback lifecycle and error handling.
- [How to handle errors and timeouts](../how-to-guides/handle-errors-and-timeouts.md)
  for what happens when a call fails.
- [Interop design](../explanation/interop-design.md) for why calls work
  this way.
