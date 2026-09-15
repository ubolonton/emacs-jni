# How to load a jar

The JVM must be `ready` before you load a jar. See
[Control the JVM lifecycle](control-the-jvm-lifecycle.md) if it is not.

## Load one jar and wait for the result

```elisp
(jvm-load-jar (expand-file-name "~/path/to/your.jar"))
;; => nil          on success
;; signals jvm-error, e.g. "jar file not found: ...", on failure
```

`jvm-load-jar` runs synchronously on the Emacs main thread. Use
`expand-file-name` to turn a relative or `~`-prefixed path into an
absolute one before passing it.

## Load a jar without blocking

```elisp
(jvm-load-jar-async "~/path/to/your.jar"
                    (lambda (ok err)
                      (if ok
                          (message "loaded")
                        (message "load failed: %s" err))))
```

`jvm-load-jar-async` returns a request ID immediately. The callback runs
later, once the JVM worker thread finishes the load.

## Load every jar in a directory

```elisp
(dolist (jar (directory-files "/path/to/jar-dir" t "\\.jar\\'"))
  (jvm-load-jar jar))
```

## Make classes from one jar visible to another

Do nothing extra. Every jar you load goes into the same shared
classloader. A class in jar A can already see a class in jar B, as
long as both are loaded, in either order.

## Set a timeout longer than the default 30 seconds

The bare `jvm-load-jar` has no timeout. To bound the wait, use the
`-await` variant, which dispatches to the worker and takes a timeout:

```elisp
(jvm-load-jar-await "~/big.jar" 120)
```

The second argument to `jvm-load-jar-await` is the timeout, in seconds.
Past it, the call signals `jvm-timeout`; a load failure signals
`jvm-error`; on success it returns nil.

## Remove a jar

There is no per-jar unload. To drop a jar, pause or stop the JVM, then
start it again with that jar left out of your loading code — see
[Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md).
