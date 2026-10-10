//! The Emacs side: code that runs on an Emacs thread with an [`emacs::Env`], or enqueues work for
//! one.

pub(crate) mod convert;
pub(crate) mod entry;
pub(crate) mod inbox;
pub(crate) mod runtime;
