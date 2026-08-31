use emacs::{defun, Env, Result, Value};

mod status;
mod protocol;

emacs::plugin_is_GPL_compatible!();

#[emacs::module(defun_prefix = "", separator = "")]
fn init(env: &Env) -> Result<Value<'_>> {
    let feature = env.intern("emacs-jni")?;
    env.call("provide", (feature,))
}

#[defun(mod_in_name = false, name = "jvm-ping")]
fn jvm_ping() -> Result<bool> {
    Ok(true)
}
