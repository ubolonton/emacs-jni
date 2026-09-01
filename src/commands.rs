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
    #[cfg(feature = "test-support")]
    TestPanic,
}
