;;; jvm-call-test.el --- Tests for jvm-call/jvm-new -*- lexical-binding: t; -*-

(require 'ert)
(require 'jvm)

(defvar jvm-call-test--calc-jar
  (expand-file-name "../target/test-fixtures/calc.jar"
                     (file-name-directory (or load-file-name buffer-file-name))))

(defun jvm-call-test--ensure-ready ()
  "Ensure the JVM is running and the Calc fixture jar is loaded.
Each test calls this itself rather than relying on a previous test having
already done so: ERT's default `t' selector enumerates tests via
`apropos-internal' over the obarray, which does not guarantee definition
order (verified empirically — this file's tests do not reliably run in
the order they appear in when driven that way), so every test must be
independently able to reach a ready JVM with the fixture jar loaded."
  (unless (eq (jvm-status) 'ready)
    (should (eq (jvm-start-await) 'ready)))
  ;; Loading the same jar into the shared classloader more than once is
  ;; harmless (`URLClassLoader#addURL' just appends to its URL list), so
  ;; this is safe to call unconditionally from every test.
  (jvm-load-jar-await jvm-call-test--calc-jar))

(ert-deftest emacs-jni-jvm-call-test/static-call-and-instance-call ()
  (jvm-call-test--ensure-ready)
  (should (equal (jvm-call-await "Calc" "addLong" '(2 3)) 5))
  (let ((obj (jvm-new-await "Calc" '(10))))
    (should (user-ptrp obj))
    (should (equal (jvm-call-await obj "addToBase" '(5)) 15))))

(ert-deftest emacs-jni-jvm-call-test/nil-marshals-to-false-for-boolean-param ()
  (jvm-call-test--ensure-ready)
  (should (equal (jvm-call-await "Calc" "describe" '(nil)) "no")))

(ert-deftest emacs-jni-jvm-call-test/bignum-round-trips ()
  (jvm-call-test--ensure-ready)
  (let ((big (* (expt 2 62) (expt 2 62)))) ; exceeds i64/long range, forces bignum
    (should (equal (jvm-call-await "Calc" "addBig" (list big 1)) (1+ big)))))

(ert-deftest emacs-jni-jvm-call-test/overload-resolution-falls-through-to-the-matching-candidate ()
  "Regression test for Finding 2 of the Phase 2 whole-branch review:
`Calc' declares two same-name, same-arity overloads --
`pick(long)' and `pick(String)' -- so `invoke_method' must try more than
just whichever candidate `getMethods()' happens to return first for a
given call to actually succeed, whichever it is."
  (jvm-call-test--ensure-ready)
  (should (equal (jvm-call-await "Calc" "pick" '("hello")) "string"))
  (should (equal (jvm-call-await "Calc" "pick" '(42)) "long")))

(ert-deftest emacs-jni-jvm-call-test/unsupported-argument-value-signals-instead-of-silently-becoming-t ()
  "A Lisp value with no marshalling rule (e.g. a vector) must signal
`jvm-invalid-argument' rather than silently being sent to Java as `true'."
  (jvm-call-test--ensure-ready)
  (should-error (jvm-call-await "Calc" "addLong" (list [1 2 3] 3))
                :type 'jvm-invalid-argument))

(ert-deftest emacs-jni-jvm-call-test/async-call-returns-keyword ()
  "The async call-done path carries a keyword result through the typed
queue (previously only the sexp reify exercised this)."
  (jvm-call-test--ensure-ready)
  ;; A Lisp callback returns :ok; Calc.callWithArg returns it; the async
  ;; call-done delivers it as an interned keyword.
  (should (eq (jvm-call-await "Calc" "callWithArg" (list (lambda (_n) :ok) 1)) :ok)))

(defun jvm-call-test--spin (n)
  "Burn N iterations of pure Lisp with no I/O.
Used only by the batch-fault-isolation regression test below, to give the
JVM worker thread wall-clock time to finish two dispatched async calls
(and `channel::poke' both) before this process's first read of the pipe
-- unlike `sleep-for'/`accept-process-output', a plain counting loop makes
no syscall, so it cannot itself trigger a process-filter run and can't
accidentally split the two completions across two reads."
  (let ((i 0)) (while (< i n) (setq i (1+ i)))))

(ert-deftest emacs-jni-jvm-call-test/batch-fault-isolation-one-callback-error-does-not-drop-sibling ()
  "Regression test for the whole-branch review's finding I1:
`lisp_requests::service_inbound' drains a whole coalesced batch of inbound
messages into one `Vec', then ran each through a `for' loop where the
`CallDone' arm used `?' -- so if `jvm--resolve-call' funcalled a
`jvm-call-async' completion callback that itself signalled (a NORMAL
event: it is arbitrary user code), the `for' loop aborted and every
message still left in that `Vec' was discarded for good (already removed
from the Rust-side queue, never re-enqueued).

This dispatches two `jvm-call-async' calls back-to-back (no intervening
`accept-process-output', so nothing yields between them) so both calls'
`Cmd::Call's run on the single worker thread and both enqueue their
`CallDone' before Emacs ever reads the pipe. `jvm-call-test--spin' then
burns enough wall-clock time (verified empirically: reliably enough that
this exact scenario -- one erroring callback ahead of one that just sets a
variable -- reproduced the pre-fix crash on every one of several manual
runs) for the worker to finish and poke for both calls before the wait
loop's first `accept-process-output', so both `CallDone's land in the
same `drain()' batch. The FIRST call's callback signals unconditionally;
before the I1 fix this crashed the whole batch process outright (an
uncaught error escaping a `:filter' function kills `emacs --batch' --
verified manually against the pre-fix code, exit status 255, no ERT
report at all) instead of leaving a clean ERT failure. After the fix, the
error is logged and the SECOND call's callback still runs, so RESULT2
below gets set. The wait loop is deadline-bounded, so if the fix ever
regresses and the two completions still land in the same batch, this
fails (or the whole process aborts, per the paragraph above) rather than
hanging forever; if they happen not to coalesce on a given run, RESULT2
still arrives from an unaffected later batch and the assertion passes
either way."
  (jvm-call-test--ensure-ready)
  (let (result2)
    (jvm-call-async "Calc" "addLong" '(1 1) (lambda (&rest _) (error "boom")))
    (jvm-call-async "Calc" "addLong" '(10 20)
                    (lambda (ok res _err) (when ok (setq result2 res))))
    (jvm-call-test--spin 300000)
    (let ((deadline (+ (float-time) 5)))
      (while (and (null result2) (< (float-time) deadline))
        (accept-process-output jvm--pipe-process 0.1)))
    (should (equal result2 30))))

(ert-deftest emacs-jni-jvm-call-test/distinct-objects-are-distinct-user-ptrs ()
  "Two separately constructed JVM objects wrap as distinct, non-`eq'
`user-ptr' values, each round-tripping independently. Supersedes the
plumbing-only `jvm--test-make-handle' check now that real method calls
produce genuine handles."
  (jvm-call-test--ensure-ready)
  (let ((a (jvm-new-await "Calc" '(1)))
        (b (jvm-new-await "Calc" '(2))))
    (should (user-ptrp a))
    (should (user-ptrp b))
    (should-not (eq a b))
    (should (equal (jvm-call-await a "addToBase" '(10)) 11))
    (should (equal (jvm-call-await b "addToBase" '(10)) 12))))

(provide 'jvm-call-test)
;;; jvm-call-test.el ends here
