//! Helpers shared by the JVM unit tests.

use jni::vm::{InitArgsBuilder, JavaVM};
use jni::JNIVersion;

/// Creates the test JVM, or attaches to the one that already exists in this process.
///
/// Retries `JavaVM::new`. jni 0.22.4 checks for an existing JVM without a lock, so parallel tests
/// can get `AlreadyCreated`; the next attempt finds the winner's JVM.
pub(crate) fn create_test_jvm() -> JavaVM {
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
