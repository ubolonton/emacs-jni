# How to load a jar

The JVM must be `ready` before you load a jar. See
[How to control the JVM lifecycle](control-the-jvm-lifecycle.md).

All jars go into one shared classloader. A class in one loaded jar can use classes in all other
loaded jars. The load order does not matter.

## Load a jar

```elisp
(jvm-load-jar (expand-file-name "~/path/to/your.jar"))
;; => nil
```

`jvm-load-jar` returns `nil` on success. It signals `jvm-error` on failure, for example
`jar/classpath entry not found: ...`. `jvm-load-jar` does not expand `~` or relative paths. Use
`expand-file-name`.

A directory of class files also works.

## Load a jar without blocking Emacs

```elisp
(jvm-load-jar-async (expand-file-name "~/path/to/your.jar")
                    (lambda (ok err)
                      (if ok
                          (message "loaded")
                        (message "load failed: %s" err))))
```

`jvm-load-jar-async` returns a request ID immediately. The callback runs after the worker thread
loads the jar.

## Load all jars in a directory

```elisp
(dolist (jar (directory-files "/path/to/jar-dir" t "\\.jar\\'"))
  (jvm-load-jar jar))
```

## Set a timeout

Use `jvm-load-jar-await`. The second argument is the timeout in seconds. The default is 30.

```elisp
(jvm-load-jar-await (expand-file-name "~/big.jar") 120)
;; => nil
```

After the timeout, `jvm-load-jar-await` signals `jvm-timeout`. A failed load signals `jvm-error`.

You cannot unload a jar. See
[Limitations and trade-offs](../explanation/limitations-and-tradeoffs.md).
