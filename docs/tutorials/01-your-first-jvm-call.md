# Your first JVM call

In this tutorial, we start a JVM inside Emacs. We call a Java method from Lisp. Then we let Java
call a Lisp function. At the end, Emacs and the JVM call each other in both directions.

This tutorial assumes a working build. If `cargo build` is new to you, read the
[Prerequisites](../../README.md#prerequisites) section of the README first.

## Step 1: Build the module and a test jar

Run these commands from the repository root:

```sh
cargo build
mkdir -p target/test-fixtures
javac -d target/test-fixtures test/fixtures/Greeter.java
jar cf target/test-fixtures/greeter.jar -C target/test-fixtures Greeter.class
```

`test/fixtures/Greeter.java` is a small class in this repository:

```java
public class Greeter {
    public static String greet() {
        return "hello from Greeter";
    }
}
```

`cargo build` makes `target/debug/libemacs_jni.so`. This is the dynamic module that
`elisp/jvm.el` loads.

## Step 2: Start Emacs with the module on the load path

```sh
emacs -Q -L elisp
```

`-Q` skips your personal init file. `-L elisp` puts `elisp/jvm.el` on the Emacs `load-path`.

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

## Step 5: Build a second fixture with a constructor and a callback method

```sh
javac -cp target/emacs-jni-shim-classes -d target/test-fixtures test/fixtures/Calc.java
jar cf target/test-fixtures/calc.jar -C target/test-fixtures Calc.class
```

`cargo build` generates the classes in `target/emacs-jni-shim-classes`. `Calc.java` needs them,
because its method `callWithArg` takes a Lisp function as a parameter. We use it in Step 7.

Load the new jar:

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
(jvm-call-await "Calc" "callWithArg" (list (lambda (n) (* n 10)) 4))
;; => 40
```

The first argument is a Lisp `lambda`. It goes into the JVM as a callable Java object.
`Calc.callWithArg` calls it with `4` and waits for the result. The Lisp function multiplied `4`
by 10, and Java returned `40` to Lisp.

A call that runs a Lisp function from Java uses `jvm-call-await`, not `jvm-call`.

We called into the JVM, and the JVM called back into Lisp.

## What's next

- [How to call Java from Lisp](../how-to-guides/call-java-from-lisp.md) for all argument types.
- [How to pass a Lisp function to Java](../how-to-guides/pass-a-lisp-function-to-java.md) for
  named functions and errors in callbacks.
- [How to handle errors and timeouts](../how-to-guides/handle-errors-and-timeouts.md) for failed
  calls.
- [Interop design](../explanation/interop-design.md) for how calls work.
