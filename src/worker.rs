use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::thread;

use crate::commands::Cmd;
use crate::jvm_thread;

static SENDER: Mutex<Option<Sender<Cmd>>> = Mutex::new(None);
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_request_id() -> u64 {
    NEXT_REQUEST_ID.fetch_add(1, Ordering::SeqCst)
}

/// Resumes the existing worker thread if one is alive (paused case), or
/// spawns a brand new one (first start, or restart after stop/crash).
pub fn start_or_resume(classpath: Vec<String>, options: Vec<String>) {
    let mut guard = SENDER.lock().unwrap();
    if let Some(sender) = guard.as_ref() {
        let _ = sender.send(Cmd::Resume);
    } else {
        let (tx, rx) = mpsc::channel();
        *guard = Some(tx);
        thread::spawn(move || jvm_thread::run(rx, classpath, options));
    }
}

/// Sends a non-Resume command to the live worker thread. Returns `Err(())`
/// if there is no worker thread currently running (shouldn't happen if
/// callers check `Status` first, but kept fallible rather than panicking).
pub fn send_command(cmd: Cmd) -> Result<(), ()> {
    let guard = SENDER.lock().unwrap();
    match guard.as_ref() {
        Some(sender) => sender.send(cmd).map_err(|_| ()),
        None => Err(()),
    }
}

/// Called by the worker thread itself right before it exits for good
/// (successful stop, or an unrecoverable panic), so the next `jvm-start`
/// knows to spawn a fresh thread instead of trying to resume a dead one.
pub fn clear_sender() {
    *SENDER.lock().unwrap() = None;
}
