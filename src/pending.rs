use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use jni::objects::JObject;
use jni::refs::Global;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
// Unlike `handles::TABLE`, entries here are bare `Global`s, not `Arc`-wrapped
// ones: a pending invocation's `CompletableFuture` global ref has exactly one
// owner at a time (this table, until `take` hands it off to
// `jvm_thread::handle_invoke_reply`), never a "resolve while still present"
// access pattern that would need cheap, repeatable cloning the way
// `handles::resolve` does -- so `Global`'s missing `Clone` impl (jni 0.22.4)
// is a non-issue here.
static TABLE: Mutex<Option<HashMap<u64, Global<JObject<'static>>>>> = Mutex::new(None);

fn table() -> std::sync::MutexGuard<'static, Option<HashMap<u64, Global<JObject<'static>>>>> {
    let mut guard = TABLE.lock().unwrap();
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    guard
}

/// Allocates a fresh correlation id for a new `invoke` round trip. Separate
/// from `register` (rather than folded into it) because
/// `shim::lisp_function_call` needs the id before it has a `Global` to
/// register -- the id goes out on the wire as part of `(invoke ID HANDLE
/// ARGS)` before the `CompletableFuture` object even exists.
pub fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::SeqCst)
}

/// Registers FUTURE (a global ref to a `java.util.concurrent.CompletableFuture`)
/// under ID, for later completion by `Cmd::InvokeReply`.
pub fn register(id: u64, future: Global<JObject<'static>>) {
    table().as_mut().unwrap().insert(id, future);
}

/// Removes and returns the pending future for ID, if any — a reply should
/// only ever resolve a given id once, so this consumes the entry rather
/// than just peeking.
pub fn take(id: u64) -> Option<Global<JObject<'static>>> {
    table().as_mut().unwrap().remove(&id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jni::vm::{InitArgsBuilder, JavaVM};
    use jni::JNIVersion;

    /// Retries against jni-rs 0.22.4's racy check-then-create singleton --
    /// see `marshal.rs`'s `create_test_jvm` for the full explanation. This
    /// test used to create its JVM directly; with more JVM-creating tests
    /// now running in this same binary (`shim.rs`), the race became
    /// reliably hit rather than a rare fluke.
    fn create_test_jvm() -> JavaVM {
        let mut last_err = None;
        for _ in 0..20 {
            let init_args = InitArgsBuilder::new().version(JNIVersion::V1_8).build().unwrap();
            match JavaVM::new(init_args) {
                Ok(vm) => return vm,
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
        panic!("failed to create/attach a test JVM after retries: {last_err:?}");
    }

    #[test]
    fn register_then_take_returns_once() {
        let vm = create_test_jvm();
        vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            let object_class = env.find_class(jni::jni_str!("java/lang/Object"))?;
            let obj = env.new_object(&object_class, jni::jni_sig!(() -> void), &[])?;
            let global = env.new_global_ref(&obj)?;
            let id = next_id();
            register(id, global);
            assert!(take(id).is_some());
            assert!(take(id).is_none());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn take_of_unknown_id_is_none() {
        assert!(take(999_999).is_none());
    }
}
