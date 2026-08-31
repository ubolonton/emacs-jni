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
            Cmd::Pause => handle_pause(&vm),
            // Handled starting in Task 6, 7 respectively.
            Cmd::Stop | Cmd::LoadJar { .. } => {}
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

fn handle_pause(vm: &JavaVM) {
    match vm.detach_current_thread() {
        Ok(()) => {
            STATUS.finish(Status::Paused);
            channel::send(&protocol::status_message(Status::Paused, None));
        }
        Err(e) => {
            // Detach failing is unexpected; stay in `pausing` rather than
            // claiming success we can't back up, and surface it as a log
            // line rather than a status (there's no `pause-failed` state
            // in the spec's state machine, so pausing simply never
            // completes — the caller's `-sync` wrapper times out).
            channel::send(&protocol::log_message(
                "error",
                &format!("jvm-pause: detach_current_thread failed: {e}"),
            ));
        }
    }
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
