use std::sync::mpsc::Receiver;

use jni::vm::{InitArgsBuilder, JavaVM};
use jni::JNIVersion;

use crate::commands::Cmd;
use crate::protocol;
use crate::status::{Status, STATUS};
use crate::{channel, worker};

/// Entry point for the JVM worker thread. Runs for the JVM's entire
/// lifetime: creates the JVM once, then loops handling commands until a
/// successful `Cmd::Stop` (or, from Task 8 on, an unrecoverable panic).
pub fn run(receiver: Receiver<Cmd>, classpath: Vec<String>, options: Vec<String>) {
    let vm = match create_jvm(&classpath, &options) {
        Ok(vm) => {
            STATUS.finish(Status::Ready);
            channel::send(&protocol::status_message(Status::Ready, None));
            vm
        }
        Err(e) => {
            STATUS.finish(Status::StartFailed);
            channel::send(&protocol::status_message(Status::StartFailed, Some(&e)));
            worker::clear_sender();
            return;
        }
    };

    for cmd in receiver {
        match cmd {
            Cmd::Resume => handle_resume(&vm),
            // Handled starting in Task 5, 6, 7 respectively. This
            // catch-all keeps the match exhaustive in the meantime; each
            // task that adds real handling replaces the corresponding
            // line here (see that task's Step for the exact diff).
            Cmd::Pause | Cmd::Stop | Cmd::LoadJar { .. } => {}
        }
    }
}

fn create_jvm(classpath: &[String], options: &[String]) -> Result<JavaVM, String> {
    let mut builder = InitArgsBuilder::new().version(JNIVersion::V1_8);
    if !classpath.is_empty() {
        builder = builder.option(format!("-Djava.class.path={}", classpath.join(":")));
    }
    for opt in options {
        builder = builder.option(opt.clone());
    }
    let init_args = builder.build().map_err(|e| e.to_string())?;
    JavaVM::new(init_args).map_err(|e| e.to_string())
}

fn handle_resume(vm: &JavaVM) {
    // Cheap: attach_current_thread is a fast thread-local check if the
    // thread is already attached, and a real (re-)attach if `Cmd::Pause`
    // (Task 5) had explicitly detached it.
    let result: jni::errors::Result<()> = vm.attach_current_thread(|_env| Ok(()));
    match result {
        Ok(()) => {
            STATUS.finish(Status::Ready);
            channel::send(&protocol::status_message(Status::Ready, None));
        }
        Err(e) => {
            STATUS.finish(Status::StartFailed);
            channel::send(&protocol::status_message(Status::StartFailed, Some(&e.to_string())));
        }
    }
}
