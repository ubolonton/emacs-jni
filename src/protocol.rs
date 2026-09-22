//! The payload types that cross the Lisp/JVM boundary — `CallTarget`,
//! `BridgeValue`, `LispOutcome`, `LispInput`, and `Status` — shared by both
//! the `lisp::` and `jvm::` sides instead of each side inventing its own
//! shape for the same value.

use crate::jvm::convert::JvmObject;

#[derive(Debug, Clone)]
pub enum CallTarget {
    /// Fully-qualified class name, for a static call or `member ==
    /// "<init>"` construction.
    ClassName(String),
    /// A previously wrapped JVM object, by its live shared reference.
    Object(JvmObject),
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
    /// A wrapped JVM object, by its live shared reference (Item 6 — no
    /// handle table).
    Object(JvmObject),
    /// A Lisp function, by its handle in the Lisp-side function table
    /// (Task 6) — auto-wrapped as `LispFunction` when this crosses into
    /// Java (Task 6).
    Callback(u64),
    /// An interned Lisp symbol or keyword, crossing as an
    /// `emacs_jni.core.LispSymbol` marker (the same class the Clojure
    /// Lisp-access path uses). `name` is the bare name (no leading `:`).
    Symbol {
        name: String,
        keyword: bool,
    },
}

/// The value a `Cmd::LispReply` or `Cmd::InvokeReply` completes a pending
/// future with — built by `lisp::inbox::service_inbound`/`jvm--invoke-
/// reply` (which run the actual `Env` op, or marshal `jvm--resolve-invoke`'s
/// result) and consumed by `jvm::inbox::handle_lisp_reply`/
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

/// One value crossing Clojure → Lisp — an `apply` receiver or argument, or
/// the operand of `typeOf`/`isNotNil`/`eq` — already classified on the JVM
/// thread (`jvm::convert::to_input`). See spec "Argument
/// conversion".
#[derive(Debug)]
pub enum LispInput {
    /// A `LispObject`, carried by its `ptr` — bound to a `Value` on the
    /// main thread, never freed here (see `borrow`).
    Obj(i64),
    /// A Clojure symbol or keyword (an `emacs_jni.core.LispSymbol` marker) —
    /// interned on the main thread. `name` is the bare name; a keyword is
    /// interned with a leading `:`.
    Symbol { name: String, keyword: bool },
    /// A scalar or other-JVM-object argument, already reduced to a
    /// `BridgeValue` the same way an ordinary `jvm-call` argument would be
    /// (reuses `jvm::convert::to_bridge`) — `BridgeValue::Callback`
    /// never appears here (that variant is only ever produced from the
    /// Lisp-to-Java direction in `lisp::convert::to_bridge`).
    Scalar(BridgeValue),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    NotStarted = 0,
    Starting = 1,
    Ready = 2,
    Pausing = 3,
    Paused = 4,
    Stopping = 5,
    Stopped = 6,
    StartFailed = 7,
    StopFailed = 8,
    Crashed = 9,
}

impl Status {
    pub(crate) fn from_u8(v: u8) -> Status {
        match v {
            0 => Status::NotStarted,
            1 => Status::Starting,
            2 => Status::Ready,
            3 => Status::Pausing,
            4 => Status::Paused,
            5 => Status::Stopping,
            6 => Status::Stopped,
            7 => Status::StartFailed,
            8 => Status::StopFailed,
            9 => Status::Crashed,
            _ => unreachable!("invalid Status discriminant"),
        }
    }

    pub fn as_tag(self) -> &'static str {
        match self {
            Status::NotStarted => "not-started",
            Status::Starting => "starting",
            Status::Ready => "ready",
            Status::Pausing => "pausing",
            Status::Paused => "paused",
            Status::Stopping => "stopping",
            Status::Stopped => "stopped",
            Status::StartFailed => "start-failed",
            Status::StopFailed => "stop-failed",
            Status::Crashed => "crashed",
        }
    }
}
