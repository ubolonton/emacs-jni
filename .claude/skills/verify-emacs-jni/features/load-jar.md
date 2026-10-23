# Load a jar

The user adds a jar or a classpath directory to the running JVM, then calls its classes. All
paths share one classloader, so a class from a jar that one path loaded is visible to the others.

## Sub-features

- `jar-direct` loads a jar on the calling thread.
- `jar-await` loads a jar on the worker and waits.
- `jar-async` loads a jar on the worker and reports to a callback `(ok err)`.
- `jar-shared` makes a class from a jar loaded on one path callable on the other paths.
- `jar-missing` rejects a path that does not exist.

## How to get to it (user POV)

- `(jvm-load-jar PATH)`: the direct path.
- `(jvm-load-jar-await PATH)` and `(jvm-load-jar-async PATH CALLBACK)`: the worker path.

## Driving it with verify-emacs

Preconditions:

- Baseline preconditions from the index. `(jvm-start)` returned `ready`.
- `$R/target/test-fixtures/calc.jar` and `greeter.jar` exist (`launch` builds them).
- No jar is loaded yet: `$V eval '(jvm-call "Calc" "addLong" (list 1 2))'` exits 1 with
  `failed to resolve Java class 'Calc' (class not found or linkage error)`.

- **Direct load.** Run `$V eval "(jvm-load-jar \"$R/target/test-fixtures/calc.jar\")"`. Prints
  `nil`.
- **Call the loaded class.** Run `$V eval '(jvm-call "Calc" "addLong" (list 40 2))'`. Prints `42`.
- **Shared loader.** Run `$V eval '(jvm-call-await "Calc" "addLong" (list 40 2))'`. Prints `42`:
  the worker path sees the jar that the direct path loaded.
- **Async load.** Run
  `$V eval "(progn (setq v--jar nil) (jvm-load-jar-async \"$R/target/test-fixtures/greeter.jar\" (lambda (ok err) (setq v--jar (list ok err)))) nil)"`,
  then poll `$V eval 'v--jar'` until it prints `(t nil)`. Then
  `$V eval '(jvm-call "Greeter" "greet" nil)'` prints `"hello from Greeter"`.
- **Missing path.** Run `$V eval '(jvm-load-jar "/no/such.jar")'`. Exit 1 with
  `*ERROR*: JVM call failed: "jar/classpath entry not found: /no/such.jar"`.
- **Proof.** The transcript holds the failed call before the load and the successful calls after
  it.

## Gotchas

- These forms use double shell quotes so that `$R` expands. Escape the inner Lisp quotes.
- `jvm-load-jar` does not expand `~` or relative paths. Give an absolute path.
- A loaded jar stays loaded for the life of the JVM. To prove a load from a clean state, use a new
  run.
