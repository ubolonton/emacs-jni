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

/// Writes a pre-formatted message (as produced by the `protocol` module) to
/// the channel. Silently drops the message if the channel hasn't been
/// initialized or the write fails — the worker thread has no way to
/// recover from a broken channel; the Elisp-side `-sync` wrappers' timeouts
/// are what catch a message that never arrives.
pub fn send(message: &str) {
    if let Some(writer) = WRITER.lock().unwrap().as_mut() {
        let _ = writer.write_all(message.as_bytes());
    }
}
