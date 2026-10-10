//! The JVM side: code that runs on a thread attached to the JVM, or enqueues work for the worker
//! thread.

pub(crate) mod convert;
pub(crate) mod entry;
pub(crate) mod inbox;
pub(crate) mod invoke;
pub(crate) mod runtime;
#[cfg(test)]
mod test_support;
