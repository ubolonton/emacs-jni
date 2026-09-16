#[derive(Debug, Clone)]
pub enum CallTarget {
    /// Fully-qualified class name, for a static call or `member ==
    /// "<init>"` construction.
    ClassName(String),
    /// A previously wrapped JVM object, by its handle in the Rust-side
    /// GlobalRef table (Task 3).
    Object(u64),
}

/// A single marshalled argument or return value, in Rust form (not yet
/// converted to a JNI `JValue` — that conversion happens in Task 4, which
/// is also the task that will likely extend this enum with more variants
/// as marshalling is implemented; keep this open to growth, don't treat
/// this exact variant set as final).
#[derive(Debug, Clone)]
pub enum LispArg {
    Nil,
    True,
    Int(i64),
    Float(f64),
    /// Decimal-digit string form of a bignum too large for `i64` — see
    /// spec "Value marshalling" > "Bignum mechanism".
    BigInt(String),
    Str(String),
    /// A previously wrapped JVM object, by handle (Task 3).
    Object(u64),
    /// A Lisp function, by its handle in the Lisp-side function table
    /// (Task 6) — auto-wrapped as `LispFunction` when this crosses into
    /// Java (Task 6).
    Callback(u64),
    /// An interned Lisp symbol or keyword, crossing as an
    /// `emacs_jni.core.LispSymbol` marker (the same class the Clojure
    /// Lisp-access path uses). `name` is the bare name (no leading `:`).
    Symbol { name: String, keyword: bool },
}

/// The value a `Cmd::LispReply` completes a pending Lisp-access future
/// with — built by `lisp_requests::service` (which runs the actual `Env`
/// op) and consumed by `jvm_thread::handle_lisp_reply` (which turns it
/// into a real Java value on the worker's own JNI attachment). Mirrors
/// `handle_invoke_reply`'s `ok`/`LispArg` pair, but as a single enum since
/// a Lisp-access reply's success shape varies by request kind (a
/// `LispObject` pointer, a `boolean`, or a `String`), unlike `invoke`'s
/// reply, which is always a `LispArg`.
#[derive(Debug)]
pub enum LispReplyValue {
    /// A boxed `emacs::GlobalRef` pointer — becomes a Java `LispObject`.
    /// Used for any result value that is not a scalar or string (symbols,
    /// conses, vectors, hash tables, ...).
    Ptr(i64),
    /// Lisp `nil` — becomes Clojure `nil` (Java `null`).
    Nil,
    Bool(bool),
    /// A fixnum — becomes a boxed `Long`.
    Int(i64),
    /// A float — becomes a boxed `Double`.
    Float(f64),
    /// Decimal digits of a bignum too large for `i64` — becomes a boxed
    /// `BigInteger`, matching the `jvm-call` marshalling table.
    BigInt(String),
    Str(String),
    /// An interned Lisp symbol or keyword — becomes an `emacs_jni.core.LispSymbol`
    /// marker the `emacs-jni.clj` namespace turns into a Clojure
    /// symbol/keyword. `name` is the bare name (no leading `:`).
    Symbol { name: String, keyword: bool },
    /// A Lisp `signal` — the future completes exceptionally with a
    /// `LispNonLocalExit(SIGNAL, symbol, data)`. `symbol` and `data` are
    /// nested, already-marshaled reply values (a symbol marker; the data
    /// list as a `LispObject`).
    Signal { symbol: Box<LispReplyValue>, data: Box<LispReplyValue> },
    /// A Lisp `throw` — `LispNonLocalExit(THROW, tag, value)`.
    Throw { tag: Box<LispReplyValue>, value: Box<LispReplyValue> },
}

#[derive(Debug)]
pub enum Cmd {
    /// Sent to an already-running worker thread to resume from `paused`.
    /// A first-time start never goes through this enum at all — see
    /// `worker::start_or_resume`, which spawns a fresh thread with
    /// classpath/options as plain arguments instead.
    Resume,
    Pause,
    Stop,
    LoadJar { id: u64, path: String },
    Call { id: u64, target: CallTarget, member: String, args: Vec<LispArg> },
    InvokeReply { id: u64, ok: bool, result: LispArg },
    ReleaseRef { handle: u64 },
    /// Completes a pending Lisp-access native's reply future (`intern`,
    /// `typeOf`, `isNotNil`, `eq`, `applyAsync`, `prin1ToString`) — sent by
    /// `jvm--service-lisp-requests` (`lisp_requests::service`) after
    /// running the corresponding `Env` op on the main thread. See
    /// `jvm_thread::handle_lisp_reply`.
    LispReply { id: u64, value: LispReplyValue },
    #[cfg(feature = "test-support")]
    TestPanic,
}
