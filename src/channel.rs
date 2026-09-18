use std::io::Write;
use std::sync::Mutex;

use emacs::{Env, Result, Value};

static WRITER: Mutex<Option<Box<dyn Write + Send>>> = Mutex::new(None);

/// Opens the channel against the given pipe-process Lisp value and stores
/// the writer globally, replacing any previous one.
pub fn init(env: &Env, process: Value) -> Result<()> {
    let writer = env.open_channel(process)?;
    *WRITER.lock().unwrap() = Some(Box::new(writer));
    Ok(())
}

/// Wakes the Emacs main thread to drain the inbound queue, with no payload.
/// The process filter (`jvm--service-inbound`) ignores the byte and drains
/// the whole queue, so coalesced wakeups are harmless. Silently drops the
/// wakeup if the channel hasn't been initialized or the write fails — the
/// worker thread has no way to recover from a broken channel; the
/// Elisp-side `-await` wrappers' timeouts are what catch a wakeup that
/// never arrives.
pub fn poke() {
    if let Some(writer) = WRITER.lock().unwrap().as_mut() {
        let _ = writer.write_all(b"\x01");
    }
}
