//! An Emacs dynamic module that embeds a JVM in the Emacs process.
//!
//! - `lisp`: the Emacs side. Module defuns, the inbound queue, and Lisp value conversion.
//! - `jvm`: the JVM side. The worker thread, reflective calls, JNI natives, and JVM value
//!   conversion.
//! - `protocol`: the types that cross between the two sides.
//!
//! See `docs/explanation/architecture.md`.

mod protocol;
mod lisp;
mod jvm;
