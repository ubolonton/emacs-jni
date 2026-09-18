use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=java/emacs_jni/core/LispCallable.java");
    println!("cargo:rerun-if-changed=java/emacs_jni/core/LispFunction.java");
    println!("cargo:rerun-if-changed=java/emacs_jni/core/LispNonLocalExit.java");
    println!("cargo:rerun-if-changed=java/emacs_jni/core/LispSignal.java");
    println!("cargo:rerun-if-changed=java/emacs_jni/core/LispThrow.java");
    println!("cargo:rerun-if-changed=java/emacs_jni/core/ReleaseNotifier.java");
    println!("cargo:rerun-if-changed=java/emacs_jni/core/LispObject.java");
    println!("cargo:rerun-if-changed=java/emacs_jni/core/Lisp.java");
    println!("cargo:rerun-if-changed=java/emacs_jni/core/LispRefReleaseNotifier.java");
    println!("cargo:rerun-if-changed=java/emacs_jni/core/LispSymbol.java");

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = Path::new(&manifest_dir).join("target/emacs-jni-shim-classes");
    std::fs::create_dir_all(&out_dir).expect("failed to create target/emacs-jni-shim-classes");

    let status = Command::new("javac")
        .arg("-d")
        .arg(&out_dir)
        .arg(format!("{manifest_dir}/java/emacs_jni/core/LispCallable.java"))
        .arg(format!("{manifest_dir}/java/emacs_jni/core/LispFunction.java"))
        .arg(format!("{manifest_dir}/java/emacs_jni/core/LispNonLocalExit.java"))
        .arg(format!("{manifest_dir}/java/emacs_jni/core/LispSignal.java"))
        .arg(format!("{manifest_dir}/java/emacs_jni/core/LispThrow.java"))
        .arg(format!("{manifest_dir}/java/emacs_jni/core/ReleaseNotifier.java"))
        .arg(format!("{manifest_dir}/java/emacs_jni/core/LispObject.java"))
        .arg(format!("{manifest_dir}/java/emacs_jni/core/Lisp.java"))
        .arg(format!("{manifest_dir}/java/emacs_jni/core/LispRefReleaseNotifier.java"))
        .arg(format!("{manifest_dir}/java/emacs_jni/core/LispSymbol.java"))
        .status()
        .expect("failed to run javac (is a JDK on PATH? see mise.toml for the pinned version)");
    assert!(status.success(), "javac failed to compile the emacs_jni.core shim");
}
