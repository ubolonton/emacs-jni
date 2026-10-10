//! Process-global JVM state, shared by the worker thread ([`crate::jvm::inbox::run`]) and the
//! Emacs main thread (`jvm--call-direct`).
//!
//! Both threads use one shared classloader ([`get_or_create_loader`]), so a direct call sees the
//! same jar classes as an async call.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::ThreadId;

use jni::objects::JObject;
use jni::refs::Global;
use jni::vm::JavaVM;

use crate::protocol::Status;

/// The shared jar classloader. [`Global`] has no [`Clone`] in jni 0.22, so it is wrapped in an
/// [`Arc`].
pub type SharedLoader = Arc<Global<JObject<'static>>>;

/// The running JVM. Set by [`set_vm`], cleared by [`clear`].
static VM: Mutex<Option<JavaVM>> = Mutex::new(None);
/// The shared jar classloader. Created by [`get_or_create_loader`], cleared by [`clear`].
static LOADER: Mutex<Option<SharedLoader>> = Mutex::new(None);

/// The Emacs main thread. Only this thread attaches to the JVM permanently, so it has a stable
/// `Thread.currentThread()` and persistent thread-locals (e.g. Clojure dynamic bindings). Other
/// threads (`make-thread`) attach for one call only. Set by [`set_main_thread`].
static MAIN_THREAD: OnceLock<ThreadId> = OnceLock::new();

/// Whether the main thread is permanently attached. Reset when the JVM stops.
static MAIN_ATTACHED: AtomicBool = AtomicBool::new(false);

/// Stores the [`JavaVM`] once the worker creates it. [`JavaVM`] is [`Clone`] in jni 0.22.
pub fn set_vm(vm: JavaVM) {
    *VM.lock().unwrap() = Some(vm);
}

/// A clone of the running [`JavaVM`], or `None` if no JVM is live.
pub fn vm() -> Option<JavaVM> {
    VM.lock().unwrap().clone()
}

/// Returns the shared classloader, creating it with `create` if none exists. `create` needs a JNI
/// `Env` on the calling thread. Serialized by the [`LOADER`] mutex, so the async and direct paths
/// cannot create two loaders.
pub fn get_or_create_loader<E>(
    create: impl FnOnce() -> Result<SharedLoader, E>,
) -> Result<SharedLoader, E> {
    let mut guard = LOADER.lock().unwrap();
    if let Some(loader) = guard.as_ref() {
        return Ok(loader.clone());
    }
    let created = create()?;
    *guard = Some(created.clone());
    Ok(created)
}

/// A clone of the shared classloader [`Arc`], or `None` if no jar was loaded.
pub fn loader() -> Option<SharedLoader> {
    LOADER.lock().unwrap().clone()
}

/// Records the Emacs main thread. Called from the module `init`. A second call keeps the first
/// id.
pub fn set_main_thread() {
    let _ = MAIN_THREAD.set(std::thread::current().id());
}

/// Whether the caller is the Emacs main thread. False before `init`.
pub fn on_main_thread() -> bool {
    MAIN_THREAD.get().is_some_and(|id| *id == std::thread::current().id())
}

/// Whether the main thread holds its permanent JVM attachment.
pub fn main_attached() -> bool {
    MAIN_ATTACHED.load(Ordering::SeqCst)
}

/// Records that the main thread now holds (`true`) or has dropped (`false`) its permanent
/// attachment.
pub fn set_main_attached(attached: bool) {
    MAIN_ATTACHED.store(attached, Ordering::SeqCst);
}

/// Clears the shared state when the JVM stops. The `jvm--stop` defun detaches the main thread
/// itself.
pub fn clear() {
    *VM.lock().unwrap() = None;
    *LOADER.lock().unwrap() = None;
    MAIN_ATTACHED.store(false, Ordering::SeqCst);
}

/// The JVM lifecycle [`Status`], readable from any thread without a lock.
///
/// A defun starts a transition with a `begin_*` method, which fails with the current status if the
/// move is not legal. The worker ends it with [`StatusCell::finish`].
pub struct StatusCell(AtomicU8);

impl StatusCell {
    /// A cell in [`Status::NotStarted`].
    pub const fn new() -> Self {
        StatusCell(AtomicU8::new(Status::NotStarted as u8))
    }

    /// The current status.
    pub fn get(&self) -> Status {
        Status::from_u8(self.0.load(Ordering::SeqCst))
    }

    fn set(&self, s: Status) {
        self.0.store(s as u8, Ordering::SeqCst);
    }

    /// Moves to `to` if the current status is in `allowed_from`. Otherwise returns the current
    /// status.
    fn try_transition(&self, allowed_from: &[Status], to: Status) -> Result<(), Status> {
        let current = self.get();
        if allowed_from.contains(&current) {
            self.set(to);
            Ok(())
        } else {
            Err(current)
        }
    }

    /// Moves to [`Status::Starting`]. Legal from not-started, paused, stopped, start-failed,
    /// stop-failed, or crashed. The caller decides between a first start and a resume.
    pub fn begin_start(&self) -> Result<(), Status> {
        self.try_transition(
            &[
                Status::NotStarted,
                Status::Paused,
                Status::Stopped,
                Status::StartFailed,
                Status::StopFailed,
                Status::Crashed,
            ],
            Status::Starting,
        )
    }

    /// Moves to [`Status::Pausing`]. Legal from ready.
    pub fn begin_pause(&self) -> Result<(), Status> {
        self.try_transition(&[Status::Ready], Status::Pausing)
    }

    /// Moves to [`Status::Stopping`]. Legal from ready or paused.
    pub fn begin_stop(&self) -> Result<(), Status> {
        self.try_transition(&[Status::Ready, Status::Paused], Status::Stopping)
    }

    /// Sets the status unconditionally. The worker calls this when a transition ends.
    pub fn finish(&self, to: Status) {
        self.set(to);
    }
}

/// The process-wide JVM status.
pub static STATUS: StatusCell = StatusCell::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_from_not_started_succeeds() {
        let cell = StatusCell::new();
        assert_eq!(cell.begin_start(), Ok(()));
        assert_eq!(cell.get(), Status::Starting);
    }

    #[test]
    fn start_from_paused_succeeds() {
        let cell = StatusCell::new();
        cell.finish(Status::Paused);
        assert_eq!(cell.begin_start(), Ok(()));
        assert_eq!(cell.get(), Status::Starting);
    }

    #[test]
    fn start_from_ready_fails() {
        let cell = StatusCell::new();
        cell.finish(Status::Ready);
        assert_eq!(cell.begin_start(), Err(Status::Ready));
        assert_eq!(cell.get(), Status::Ready);
    }

    #[test]
    fn pause_from_ready_succeeds() {
        let cell = StatusCell::new();
        cell.finish(Status::Ready);
        assert_eq!(cell.begin_pause(), Ok(()));
        assert_eq!(cell.get(), Status::Pausing);
    }

    #[test]
    fn pause_from_not_started_fails() {
        let cell = StatusCell::new();
        assert_eq!(cell.begin_pause(), Err(Status::NotStarted));
    }

    #[test]
    fn stop_from_ready_succeeds() {
        let cell = StatusCell::new();
        cell.finish(Status::Ready);
        assert_eq!(cell.begin_stop(), Ok(()));
        assert_eq!(cell.get(), Status::Stopping);
    }

    #[test]
    fn stop_from_paused_succeeds() {
        let cell = StatusCell::new();
        cell.finish(Status::Paused);
        assert_eq!(cell.begin_stop(), Ok(()));
        assert_eq!(cell.get(), Status::Stopping);
    }

    #[test]
    fn stop_from_not_started_fails() {
        let cell = StatusCell::new();
        assert_eq!(cell.begin_stop(), Err(Status::NotStarted));
    }

    #[test]
    fn as_tag_matches_spec_names() {
        assert_eq!(Status::NotStarted.as_tag(), "not-started");
        assert_eq!(Status::StartFailed.as_tag(), "start-failed");
        assert_eq!(Status::StopFailed.as_tag(), "stop-failed");
        assert_eq!(Status::Crashed.as_tag(), "crashed");
    }
}
