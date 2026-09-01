use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=java/emacs_jni/LispCallable.java");
    println!("cargo:rerun-if-changed=java/emacs_jni/LispFunction.java");
    println!("cargo:rerun-if-changed=java/emacs_jni/LispError.java");

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = Path::new(&manifest_dir).join("target/emacs-jni-shim-classes");
    std::fs::create_dir_all(&out_dir).expect("failed to create target/emacs-jni-shim-classes");

    let status = Command::new("javac")
        .arg("-d")
        .arg(&out_dir)
        .arg(format!("{manifest_dir}/java/emacs_jni/LispCallable.java"))
        .arg(format!("{manifest_dir}/java/emacs_jni/LispFunction.java"))
        .arg(format!("{manifest_dir}/java/emacs_jni/LispError.java"))
        .status()
        .expect("failed to run javac (is a JDK on PATH? see mise.toml for the pinned version)");
    assert!(status.success(), "javac failed to compile the emacs_jni shim");
}
