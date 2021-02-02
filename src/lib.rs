#![allow(non_upper_case_globals)]

use std::fs::OpenOptions;
use gag::{Gag, Redirect};
use once_cell::sync::OnceCell;
use once_cell::unsync::{OnceCell as TOnceCell};

use jni::{InitArgsBuilder, JNIVersion, JavaVM};

use emacs::{Env, Result, defun};
use jni::objects::{JValue, JClass, JStaticMethodID};
use jni::signature::{JavaType, Primitive};
use jni::descriptors::Desc;

emacs::plugin_is_GPL_compatible! {}

static JVM: OnceCell<JavaVM> = OnceCell::new();

#[emacs::module(defun_prefix = "jni")]
fn init(_: &Env) -> Result<()> {
    // let log = OpenOptions::new()
    //     .truncate(true)
    //     .read(true)
    //     .create(true)
    //     .write(true)
    //     .open("/tmp/emacs-jni.log")
    //     .unwrap();

    // Redirect::stderr(log).unwrap();

    Gag::stderr().unwrap();

    // Build the VM properties
    let jvm_args = InitArgsBuilder::new()
        // Pass the JNI API version (default is 8)
        .version(JNIVersion::V8)
        // Extra JNI checks useful during development
        .option("-Xcheck:jni")
        // .option("-XX:+AllowUserSignalHandlers")
        .option("-Xrs")
        .build()
        .unwrap();

    // Create a new VM
    JVM.get_or_try_init(|| JavaVM::new(jvm_args))?;

    let j_env = JVM.get().unwrap().attach_current_thread_permanently()?;

    j_math.with(|m| {
        eprintln!("Getting java.lang.Math");
        m.get_or_try_init(|| "java/lang/Math".lookup(&j_env)).map(|m| m.clone())
    })?;

    j_sin.with(|m| {
        eprintln!("Getting java.lang.Math.sin");
        m.get_or_try_init(|| ("java/lang/Math", "sin", "(D)D").lookup(&j_env)).map(|m| m.clone())
    })?;

    eprintln!("Done initializing");

    Ok(())
}

#[defun]
fn abs(x: i32) -> Result<i32> {
    let jvm = JVM.get().unwrap();
    // let j_env = jvm.attach_current_thread()?;
    let j_env = jvm.get_env()?;

    let x = JValue::from(x);
    let val = j_env.call_static_method("java/lang/Math", "abs", "(I)I", &[x])?
        .i()?;

    Ok(val)
}

thread_local! {
    static j_math: TOnceCell<JClass<'static>> = TOnceCell::new();
    static j_sin: TOnceCell<JStaticMethodID<'static>> = TOnceCell::new();
}

#[defun]
fn sin(x: f64) -> Result<f64> {
    let jvm = JVM.get().unwrap();
    // let j_env = jvm.attach_current_thread()?;
    let j_env = jvm.get_env()?;
    let x = JValue::from(x);
    let val = j_env.call_static_method_unchecked(
        j_math.with(|m| m.get().unwrap().clone()),
        j_sin.with(|m| m.get().unwrap().clone()),
        JavaType::Primitive(Primitive::Double),
        &[x]
    )?.d()?;

    // let class = MATH.get_or_try_init(|| "java/lang/Math".lookup(&j_env))?;

    // let class = "java/lang/Math".lookup(&j_env)?;
    // let name = "sin".lookup(&j_env)?;

    Ok(val)
}

#[defun]
fn abs_unchecked(x: i32) -> Result<i32> {
    let jvm = JVM.get().unwrap();
    // let j_env = jvm.attach_current_thread()?;
    let j_env = jvm.get_env()?;

    let x = JValue::from(x);
    let val = j_env.call_static_method_unchecked(
        "java/lang/Math",
        ("java/lang/Math", "abs", "(I)I"),
        JavaType::Primitive(Primitive::Int),
        &[x],
    )?
        .i()?;

    Ok(val)
}
