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
pub enum BridgeValue {
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

/// The value a `Cmd::LispReply` or `Cmd::InvokeReply` completes a pending
/// future with — built by `lisp_requests::service_inbound`/`jvm--invoke-
/// reply` (which run the actual `Env` op, or marshal `jvm--resolve-invoke`'s
/// result) and consumed by `jvm_thread::handle_lisp_reply`/
/// `handle_invoke_reply` (which turn it into a real Java value, or a thrown
/// `LispSignal`/`LispThrow`, on the worker's own JNI attachment) via the
/// shared `complete_future_with_reply`. A single enum since a reply's
/// success shape varies by request kind (a `LispObject` pointer, a
/// `boolean`, a `String`, ...), and a failure carries the real Lisp signal
/// symbol/data or throw tag/value, not a flattened message.
#[derive(Debug)]
pub enum LispOutcome {
    /// A scalar or symbol result, reusing the generic bridge carrier — the
    /// dedup'd overlap with `BridgeValue` (was `Nil`/`Int`/`Float`/`BigInt`/
    /// `Str`/`Symbol`). `nil` is `BridgeValue::Nil`, the symbol `t` stays
    /// `Bool(true)` below (a Lisp predicate result usable as a Clojure boolean).
    Value(BridgeValue),
    /// A computed boolean from `eq`/`is-not-nil?`, or the Lisp symbol `t` —
    /// always a real Java `Boolean`, never `nil`/null.
    Bool(bool),
    /// A boxed `emacs::GlobalRef` pointer — an opaque Lisp `LispObject`.
    Ptr(i64),
    /// A Lisp `signal`: completes exceptionally with a `LispSignal`.
    Signal { symbol: Box<LispOutcome>, data: Box<LispOutcome> },
    /// A Lisp `throw`: completes exceptionally with a `LispThrow`.
    Throw { tag: Box<LispOutcome>, value: Box<LispOutcome> },
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
    Call { id: u64, target: CallTarget, member: String, args: Vec<BridgeValue> },
    /// Completes a pending `LispFunction.call` future (`shim::
    /// lisp_function_call`) with the Lisp callback's outcome — sent by
    /// `jvm--invoke-reply` once `jvm--resolve-invoke` (`elisp/jvm.el`) has
    /// funcalled the callback and marshalled its return value or its real
    /// `condition-case` signal into a `LispOutcome`. See
    /// `jvm_thread::handle_invoke_reply`.
    InvokeReply { id: u64, outcome: LispOutcome },
    ReleaseRef { handle: u64 },
    /// Completes a pending Lisp-access native's reply future (`intern`,
    /// `typeOf`, `isNotNil`, `eq`, `applyAsync`, `prin1ToString`) — sent by
    /// `jvm--service-lisp-requests` (`lisp_requests::service`) after
    /// running the corresponding `Env` op on the main thread. See
    /// `jvm_thread::handle_lisp_reply`.
    LispReply { id: u64, value: LispOutcome },
    #[cfg(feature = "test-support")]
    TestPanic,
}
