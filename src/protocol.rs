//! Types that cross the Lisp/JVM boundary, shared by `lisp::` and `jvm::`.

use crate::jvm::convert::JvmObject;

#[derive(Debug, Clone)]
pub enum CallTarget {
    /// Fully-qualified class name, for a static call or `<init>`.
    ClassName(String),
    /// A wrapped JVM object.
    Object(JvmObject),
}

/// A marshalled argument or return value, before conversion to a JNI or Lisp value.
#[derive(Debug, Clone)]
pub enum BridgeValue {
    Nil,
    True,
    Int(i64),
    Float(f64),
    /// Decimal digits of an integer too large for `i64`.
    BigInt(String),
    Str(String),
    /// A wrapped JVM object.
    Object(JvmObject),
    /// A handle into Lisp's function table (`jvm--lisp-callbacks`). Becomes a `LispFunction` in
    /// Java.
    Callback(u64),
    /// A symbol or keyword. Becomes an `emacs_jni.core.LispSymbol` in Java. `name` has no
    /// leading `:`.
    Symbol {
        name: String,
        keyword: bool,
    },
}

/// The result of a Lisp-side operation requested by Java, carried by `Cmd::LispReply` or
/// `Cmd::InvokeReply`. A failure carries the original signal symbol/data or throw tag/value.
#[derive(Debug)]
pub enum LispOutcome {
    /// A scalar or symbol. `nil` is `BridgeValue::Nil`; `t` is `Bool(true)`.
    Value(BridgeValue),
    /// A boolean result (`eq`, `is-not-nil?`), or `t`. Always a Java `Boolean`, never null.
    Bool(bool),
    /// A boxed `emacs::GlobalRef` pointer: an opaque `LispObject`.
    Ptr(i64),
    /// A Lisp `signal`: completes exceptionally with a `LispSignal`.
    Signal { symbol: Box<LispOutcome>, data: Box<LispOutcome> },
    /// A Lisp `throw`: completes exceptionally with a `LispThrow`.
    Throw { tag: Box<LispOutcome>, value: Box<LispOutcome> },
}

/// One value crossing Clojure → Lisp: an `apply` function or argument, or the operand of
/// `typeOf`/`isNotNil`/`eq`. Classified on the JVM thread by `jvm::convert::to_input`.
#[derive(Debug)]
pub enum LispInput {
    /// A `LispObject`'s `ptr`. Borrowed, not freed (see `lisp::inbox::borrow`).
    Obj(i64),
    /// A `LispSymbol`, interned on the main thread. `name` has no leading `:`.
    Symbol { name: String, keyword: bool },
    /// Any other value, converted like a `jvm-call` argument. Never `BridgeValue::Callback`.
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
