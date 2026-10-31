//! Types that cross the Lisp/JVM boundary, shared by [`crate::lisp`] and [`crate::jvm`].

use crate::jvm::convert::JvmObject;

/// The `kind` of a Lisp `signal` that crosses to the JVM. Mirrors Java `LispNonLocalExit.SIGNAL`.
pub(crate) const NON_LOCAL_EXIT_SIGNAL: i32 = 0;
/// The `kind` of a Lisp `throw` that crosses to the JVM. Mirrors Java `LispNonLocalExit.THROW`.
pub(crate) const NON_LOCAL_EXIT_THROW: i32 = 1;

/// The receiver of a `jvm-call`: a class for a static call or a constructor, or an object for an
/// instance call.
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
    /// Java. Flows Lisp to Java only.
    Callback(u64),
    /// A symbol or keyword. Becomes an `emacs_jni.core.LispSymbol` in Java. `name` has no
    /// leading `:`.
    Symbol {
        name: String,
        keyword: bool,
    },
}

/// The result of a Lisp-side operation requested by Java, carried by
/// [`Cmd::LispReply`](crate::jvm::inbox::Cmd::LispReply) or
/// [`Cmd::InvokeReply`](crate::jvm::inbox::Cmd::InvokeReply). A failure carries the original
/// signal symbol/data or throw tag/value.
#[derive(Debug)]
pub enum LispOutcome {
    /// A scalar or symbol. `nil` is [`BridgeValue::Nil`]; `t` is [`LispOutcome::Bool`]`(true)`.
    Value(BridgeValue),
    /// A boolean result (`eq`, `is-not-nil?`), or `t`. Always a Java `Boolean`, never null.
    Bool(bool),
    /// A boxed `emacs::GlobalRef` pointer: an opaque `LispObject`.
    Ptr(i64),
    /// A Lisp `signal`: completes the Java future exceptionally with a `LispSignal`. `message`
    /// becomes its Java message (see [`crate::lisp::convert::signal_message`]).
    Signal { symbol: Box<LispOutcome>, data: Box<LispOutcome>, message: Option<String> },
    /// A Lisp `throw`: completes the Java future exceptionally with a `LispThrow`. `message`
    /// becomes its Java message (see [`crate::lisp::convert::throw_message`]).
    Throw { tag: Box<LispOutcome>, value: Box<LispOutcome>, message: Option<String> },
}

/// One value crossing Clojure → Lisp: an `apply` function or argument, or the operand of
/// `typeOf`/`isNotNil`/`eq`. Classified on the JVM thread by [`crate::jvm::convert::to_input`].
#[derive(Debug)]
pub enum LispInput {
    /// A `LispObject`'s `ptr`. Borrowed, not freed (see [`crate::lisp::inbox::borrow`]).
    Obj(i64),
    /// A `LispSymbol`, interned on the main thread. `name` has no leading `:`.
    Symbol { name: String, keyword: bool },
    /// Any other value, converted like a `jvm-call` argument. Never [`BridgeValue::Callback`].
    Scalar(BridgeValue),
}

/// The JVM lifecycle state. Stored in [`crate::jvm::runtime::STATUS`] as a `u8`, and shown to Lisp
/// as the symbol from [`Status::as_tag`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    /// No start was requested in this process.
    NotStarted = 0,
    /// The worker is creating the JVM, or resuming after a pause.
    Starting = 1,
    /// The JVM accepts calls.
    Ready = 2,
    /// The worker is detaching from the JVM.
    Pausing = 3,
    /// The worker is detached. The JVM still exists, so a start resumes it.
    Paused = 4,
    /// The worker is destroying the JVM.
    Stopping = 5,
    /// The JVM was destroyed.
    Stopped = 6,
    /// JVM creation or resume failed.
    StartFailed = 7,
    /// JVM destruction failed.
    StopFailed = 8,
    /// The worker thread panicked.
    Crashed = 9,
}

impl Status {
    /// Converts a stored discriminant back to a [`Status`]. Only values from `as u8` are valid.
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

    /// The Lisp symbol name for this state, e.g. `"start-failed"`. `jvm--status` holds it.
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
