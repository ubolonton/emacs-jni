use crate::jvm::convert::JvmObject;
use crate::protocol::{BridgeValue, CallTarget, LispOutcome};

#[derive(Debug)]
pub enum Cmd {
    /// Sent to an already-running worker thread to resume from `paused`.
    /// A first-time start never goes through this enum at all — see
    /// `worker::start_or_resume`, which spawns a fresh thread with
    /// classpath/options as plain arguments instead.
    Resume,
    Pause,
    Stop,
    LoadJar {
        id: u64,
        path: String,
    },
    Call {
        id: u64,
        target: CallTarget,
        member: String,
        args: Vec<BridgeValue>,
    },
    /// Completes a pending `LispFunction.call` future (`shim::
    /// lisp_function_call`) with the Lisp callback's outcome — sent by
    /// `jvm--invoke-reply` once `jvm--resolve-invoke` (`elisp/jvm.el`) has
    /// funcalled the callback and marshalled its return value or its real
    /// `condition-case` signal into a `LispOutcome`. See
    /// `jvm_thread::handle_invoke_reply`.
    InvokeReply {
        id: u64,
        outcome: LispOutcome,
    },
    /// A Lisp `user-ptr` wrapping a JVM object was GC'd; drop its reference
    /// on the attached worker thread (see `jvm::convert::JvmHandle::Drop`).
    ReleaseRef {
        object: JvmObject,
    },
    /// Completes a pending Lisp-access native's reply future (`intern`,
    /// `typeOf`, `isNotNil`, `eq`, `applyAsync`, `prin1ToString`) — sent by
    /// `jvm--service-inbound` (`lisp_requests::service_inbound`) after
    /// running the corresponding `Env` op on the main thread. See
    /// `jvm_thread::handle_lisp_reply`.
    LispReply {
        id: u64,
        value: LispOutcome,
    },
    #[cfg(feature = "test-support")]
    TestPanic,
}
