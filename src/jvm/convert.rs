//! JVM value conversion, on a thread attached to the JVM.
//!
//! - Into Java ([`from_bridge`]): type-directed for numbers. The declared Java parameter type
//!   selects the Java form of a number. Other values have one Java form.
//! - Out of Java ([`to_bridge`], [`to_input`]): value-directed. The runtime class of the object
//!   selects the [`BridgeValue`] or [`LispInput`].

use std::sync::Arc;

use jni::objects::{JClass, JObject, JObjectArray, JString, JValueOwned};
use jni::refs::Global;

use crate::protocol::{BridgeValue, LispInput};

/// Converts `arg` to a JNI value for a parameter of type `param_type`. The mapping depends on the
/// declared type, because `Method.invoke` allows only widening primitive conversions. See
/// `docs/explanation/interop-design.md`, "Why marshalling is type-directed, not value-directed".
pub fn from_bridge<'local>(
    env: &mut jni::Env<'local>,
    arg: &BridgeValue,
    param_type: &JClass<'local>,
) -> jni::errors::Result<JValueOwned<'local>> {
    let param_name = class_name(env, param_type)?;

    match (arg, param_name.as_str()) {
        (BridgeValue::Nil, "boolean") | (BridgeValue::Nil, "java.lang.Boolean") => {
            Ok(JValueOwned::Bool(false))
        }
        (BridgeValue::Nil, _) => Ok(JValueOwned::Object(JObject::null())),
        (BridgeValue::True, _) => Ok(JValueOwned::Bool(true)),
        (BridgeValue::Int(n), "int") => i32::try_from(*n).map(JValueOwned::Int).map_err(|_| {
            marshal_error(&format!("integer {n} out of range for parameter type int"))
        }),
        (BridgeValue::Int(n), "short") => i16::try_from(*n).map(JValueOwned::Short).map_err(|_| {
            marshal_error(&format!("integer {n} out of range for parameter type short"))
        }),
        (BridgeValue::Int(n), "byte") => i8::try_from(*n).map(JValueOwned::Byte).map_err(|_| {
            marshal_error(&format!("integer {n} out of range for parameter type byte"))
        }),
        // `java.lang.Object` targets get `long`/`double`/`BigInteger`, matching what
        // `object_to_bridge` produces. This keeps Java→Lisp→Java round trips lossless.
        // `handle_invoke_reply` relies on it, because `CompletableFuture#complete` takes `Object`.
        (BridgeValue::Int(n), "long") | (BridgeValue::Int(n), "java.lang.Object") => {
            Ok(JValueOwned::Long(*n))
        }
        (BridgeValue::Int(n), "float") => Ok(JValueOwned::Float(*n as f32)),
        (BridgeValue::Int(n), "double") => Ok(JValueOwned::Double(*n as f64)),
        (BridgeValue::Int(n), "java.math.BigInteger") => big_integer_from_str(env, &n.to_string()),
        (BridgeValue::Int(_), other) => {
            Err(marshal_error(&format!("integer argument doesn't fit parameter type {other}")))
        }
        (BridgeValue::Float(f), "float") => Ok(JValueOwned::Float(*f as f32)),
        (BridgeValue::Float(f), "double") | (BridgeValue::Float(f), "java.lang.Object") => {
            Ok(JValueOwned::Double(*f))
        }
        (BridgeValue::Float(_), other) => {
            Err(marshal_error(&format!("float argument doesn't fit parameter type {other}")))
        }
        (BridgeValue::BigInt(digits), "java.math.BigInteger")
        | (BridgeValue::BigInt(digits), "java.lang.Object") => big_integer_from_str(env, digits),
        (BridgeValue::BigInt(_), other) => {
            Err(marshal_error(&format!("bignum argument too large for parameter type {other}")))
        }
        (BridgeValue::Str(s), _) => {
            let jstr = env.new_string(s)?;
            Ok(JValueOwned::Object(jstr.into()))
        }
        (BridgeValue::Object(object), _) => {
            let local = env.new_local_ref(object.0.as_obj())?;
            Ok(JValueOwned::Object(local))
        }
        // Lisp already registered `handle` in `jvm--lisp-callbacks`. Java calls back through it.
        (BridgeValue::Callback(handle), _) => {
            let lisp_function_class =
                env.find_class(jni::jni_str!("emacs_jni/core/LispFunction"))?;
            let obj = env.new_object(
                &lisp_function_class,
                jni::jni_sig!((handle: long) -> void),
                &[JValueOwned::Long(*handle as i64).borrow()],
            )?;
            register_release_on_gc(env, &obj, *handle)?;
            Ok(JValueOwned::Object(obj))
        }
        // Same construction as `jvm::invoke::build_lisp_reply_object`, so both directions produce
        // the same `LispSymbol`.
        (BridgeValue::Symbol { name, keyword }, _) => {
            let jname = env.new_string(name)?;
            let class = env.find_class(jni::jni_str!("emacs_jni/core/LispSymbol"))?;
            let obj = env.new_object(
                &class,
                jni::jni_sig!((name: java.lang.String, keyword: boolean) -> void),
                &[JValueOwned::Object(jname.into()).borrow(), JValueOwned::Bool(*keyword).borrow()],
            )?;
            Ok(JValueOwned::Object(obj))
        }
    }
}

/// Registers a `Cleaner` action on `obj`, a new `LispFunction`, that sends
/// [`Inbound::Release`](crate::lisp::inbox::Inbound::Release) for `handle` after `obj` is
/// collected. Lisp then drops the callback from its table. The action runs later, on a JVM
/// background thread.
///
/// Done here, not in `LispFunction`, so that the action can reuse the `ReleaseNotifier` shim
/// instead of adding a second native method to `LispFunction`.
fn register_release_on_gc<'local>(
    env: &mut jni::Env<'local>,
    obj: &JObject<'local>,
    handle: u64,
) -> jni::errors::Result<()> {
    let notifier_class = env.find_class(jni::jni_str!("emacs_jni/core/ReleaseNotifier"))?;
    let notifier = env.new_object(
        &notifier_class,
        jni::jni_sig!((handle: long) -> void),
        &[JValueOwned::Long(handle as i64).borrow()],
    )?;

    let cleaner_class = env.find_class(jni::jni_str!("java/lang/ref/Cleaner"))?;
    // `ref` is a Rust keyword, so `jni_sig!` needs the quoted form here. Nested classes use `$`.
    let cleaner = env
        .call_static_method(
            &cleaner_class,
            jni::jni_str!("create"),
            jni::jni_sig!(() -> "java.lang.ref.Cleaner"),
            &[],
        )?
        .l()?;
    // `JValue::Object` borrows, so this needs no extra local refs.
    env.call_method(
        &cleaner,
        jni::jni_str!("register"),
        jni::jni_sig!((obj: java.lang.Object, action: java.lang.Runnable) -> "java.lang.ref.Cleaner$Cleanable"),
        &[jni::objects::JValue::Object(obj), jni::objects::JValue::Object(&notifier)],
    )?;
    Ok(())
}

/// Makes a `java.math.BigInteger` from decimal `digits`.
fn big_integer_from_str<'local>(
    env: &mut jni::Env<'local>,
    digits: &str,
) -> jni::errors::Result<JValueOwned<'local>> {
    let jstr = env.new_string(digits)?;
    let big_integer_class = env.find_class(jni::jni_str!("java/math/BigInteger"))?;
    let obj = env.new_object(
        &big_integer_class,
        jni::jni_sig!((val: java.lang.String) -> void),
        &[JValueOwned::Object(jstr.into()).borrow()],
    )?;
    Ok(JValueOwned::Object(obj))
}

/// Returns the Java-source name of `class`, e.g. `"int"` or `"java.lang.String"`.
fn class_name(env: &mut jni::Env<'_>, class: &JClass<'_>) -> jni::errors::Result<String> {
    let name = env
        .call_method(class, jni::jni_str!("getName"), jni::jni_sig!(() -> java.lang.String), &[])?
        .l()?;
    let name = env.cast_local::<jni::objects::JString>(name)?;
    Ok(name.to_string())
}

/// Wraps an application error message as a [`jni::errors::Error`].
/// [`ParseFailed`](jni::errors::Error::ParseFailed) is the only variant that takes an owned
/// message, so the name does not match these errors.
pub(crate) fn marshal_error(msg: &str) -> jni::errors::Error {
    jni::errors::Error::ParseFailed(msg.to_string())
}

/// Boxes a primitive [`JValueOwned`] for an `Object[]` argument to `Method.invoke` or
/// `Constructor.newInstance`. Objects (including `null`) pass through.
pub(crate) fn box_if_primitive<'local>(
    env: &mut jni::Env<'local>,
    value: JValueOwned<'local>,
) -> jni::errors::Result<JObject<'local>> {
    match value {
        JValueOwned::Object(o) => Ok(o),
        JValueOwned::Bool(b) => box_via_value_of(
            env,
            jni::jni_str!("java/lang/Boolean"),
            jni::jni_sig!((v: boolean) -> java.lang.Boolean),
            JValueOwned::Bool(b),
        ),
        JValueOwned::Byte(n) => box_via_value_of(
            env,
            jni::jni_str!("java/lang/Byte"),
            jni::jni_sig!((v: byte) -> java.lang.Byte),
            JValueOwned::Byte(n),
        ),
        JValueOwned::Short(n) => box_via_value_of(
            env,
            jni::jni_str!("java/lang/Short"),
            jni::jni_sig!((v: short) -> java.lang.Short),
            JValueOwned::Short(n),
        ),
        JValueOwned::Int(n) => box_via_value_of(
            env,
            jni::jni_str!("java/lang/Integer"),
            jni::jni_sig!((v: int) -> java.lang.Integer),
            JValueOwned::Int(n),
        ),
        JValueOwned::Long(n) => box_via_value_of(
            env,
            jni::jni_str!("java/lang/Long"),
            jni::jni_sig!((v: long) -> java.lang.Long),
            JValueOwned::Long(n),
        ),
        JValueOwned::Float(f) => box_via_value_of(
            env,
            jni::jni_str!("java/lang/Float"),
            jni::jni_sig!((v: float) -> java.lang.Float),
            JValueOwned::Float(f),
        ),
        JValueOwned::Double(f) => box_via_value_of(
            env,
            jni::jni_str!("java/lang/Double"),
            jni::jni_sig!((v: double) -> java.lang.Double),
            JValueOwned::Double(f),
        ),
        JValueOwned::Char(c) => box_via_value_of(
            env,
            jni::jni_str!("java/lang/Character"),
            jni::jni_sig!((v: char) -> java.lang.Character),
            JValueOwned::Char(c),
        ),
        JValueOwned::Void => Ok(JObject::null()),
    }
}

/// Boxes a primitive with the static `valueOf` method of `class_name`, e.g. `Long.valueOf`.
/// Generic over `S` because that is what [`jni::jni_sig!`] expands to.
fn box_via_value_of<'local, 'sig, 'sig_args, S>(
    env: &mut jni::Env<'local>,
    class_name: impl AsRef<jni::strings::JNIStr>,
    sig: S,
    value: JValueOwned<'local>,
) -> jni::errors::Result<JObject<'local>>
where
    S: AsRef<jni::signature::MethodSignature<'sig, 'sig_args>>,
{
    let class = env.find_class(class_name)?;
    env.call_static_method(&class, jni::jni_str!("valueOf"), sig, &[value.borrow()])?.l()
}

/// Converts a JNI return value to a [`BridgeValue`], by its runtime class. A non-scalar Java
/// object becomes [`BridgeValue::Object`] with a new global reference.
pub fn to_bridge<'local>(
    env: &mut jni::Env<'local>,
    value: JValueOwned<'local>,
) -> jni::errors::Result<BridgeValue> {
    match value {
        JValueOwned::Void => Ok(BridgeValue::Nil),
        JValueOwned::Bool(b) => Ok(if b { BridgeValue::True } else { BridgeValue::Nil }),
        JValueOwned::Byte(n) => Ok(BridgeValue::Int(n as i64)),
        JValueOwned::Short(n) => Ok(BridgeValue::Int(n as i64)),
        JValueOwned::Int(n) => Ok(BridgeValue::Int(n as i64)),
        JValueOwned::Long(n) => Ok(BridgeValue::Int(n)),
        JValueOwned::Float(f) => Ok(BridgeValue::Float(f as f64)),
        JValueOwned::Double(f) => Ok(BridgeValue::Float(f)),
        JValueOwned::Char(c) => Ok(BridgeValue::Int(c as i64)),
        JValueOwned::Object(obj) => object_to_bridge(env, obj),
    }
}

/// Converts a Java object to a [`BridgeValue`], for [`to_bridge`]. Checks `String`, `Boolean`,
/// `BigInteger`, the integral wrappers, `Number`, then `LispSymbol`, in that order. Any other
/// object becomes [`BridgeValue::Object`].
fn object_to_bridge<'local>(
    env: &mut jni::Env<'local>,
    obj: JObject<'local>,
) -> jni::errors::Result<BridgeValue> {
    if obj.is_null() {
        return Ok(BridgeValue::Nil);
    }

    let string_class = env.find_class(jni::jni_str!("java/lang/String"))?;
    let boolean_class = env.find_class(jni::jni_str!("java/lang/Boolean"))?;
    let big_integer_class = env.find_class(jni::jni_str!("java/math/BigInteger"))?;
    // `Method.invoke` boxes every primitive return, so this is the common numeric path. Check the
    // integral wrappers before `Number`: `doubleValue()` would lose precision above 2^53.
    let integer_class = env.find_class(jni::jni_str!("java/lang/Integer"))?;
    let long_class = env.find_class(jni::jni_str!("java/lang/Long"))?;
    let short_class = env.find_class(jni::jni_str!("java/lang/Short"))?;
    let byte_class = env.find_class(jni::jni_str!("java/lang/Byte"))?;
    let number_class = env.find_class(jni::jni_str!("java/lang/Number"))?;

    if env.is_instance_of(&obj, &string_class)? {
        let s = env.cast_local::<jni::objects::JString>(obj)?;
        return Ok(BridgeValue::Str(s.to_string()));
    }
    if env.is_instance_of(&obj, &boolean_class)? {
        let b = env
            .call_method(&obj, jni::jni_str!("booleanValue"), jni::jni_sig!(() -> boolean), &[])?
            .z()?;
        return Ok(if b { BridgeValue::True } else { BridgeValue::Nil });
    }
    if env.is_instance_of(&obj, &big_integer_class)? {
        let s = env
            .call_method(
                &obj,
                jni::jni_str!("toString"),
                jni::jni_sig!(() -> java.lang.String),
                &[],
            )?
            .l()?;
        let s = env.cast_local::<jni::objects::JString>(s)?;
        return Ok(BridgeValue::BigInt(s.to_string()));
    }
    if env.is_instance_of(&obj, &integer_class)?
        || env.is_instance_of(&obj, &long_class)?
        || env.is_instance_of(&obj, &short_class)?
        || env.is_instance_of(&obj, &byte_class)?
    {
        let n = env
            .call_method(&obj, jni::jni_str!("longValue"), jni::jni_sig!(() -> long), &[])?
            .j()?;
        return Ok(BridgeValue::Int(n));
    }
    if env.is_instance_of(&obj, &number_class)? {
        let d = env
            .call_method(&obj, jni::jni_str!("doubleValue"), jni::jni_sig!(() -> double), &[])?
            .d()?;
        return Ok(BridgeValue::Float(d));
    }

    let lisp_symbol_class = env.find_class(jni::jni_str!("emacs_jni/core/LispSymbol"))?;
    if env.is_instance_of(&obj, &lisp_symbol_class)? {
        let name =
            env.get_field(&obj, jni::jni_str!("name"), jni::jni_sig!(java.lang.String))?.l()?;
        let name = env.cast_local::<jni::objects::JString>(name)?.to_string();
        let keyword = env.get_field(&obj, jni::jni_str!("keyword"), jni::jni_sig!(boolean))?.z()?;
        return Ok(BridgeValue::Symbol { name, keyword });
    }

    let global = env.new_global_ref(&obj)?;
    Ok(BridgeValue::Object(JvmObject(Arc::new(global))))
}

/// A global reference to a JVM object, shared by [`Arc`] clone. [`Global`] has no [`Clone`] in jni
/// 0.22: a real clone needs an `Env`.
#[derive(Debug, Clone)]
pub struct JvmObject(pub Arc<Global<JObject<'static>>>);

/// The Lisp `user-ptr` for a JVM object. Each JVM→Lisp crossing makes a new [`Global`]; the
/// wrapper is never cloned.
pub struct JvmHandle(pub JvmObject);

impl emacs::Transfer for JvmHandle {}

impl Drop for JvmHandle {
    fn drop(&mut self) {
        // Lisp's GC may run this on a thread with no JNI attachment. Send a clone to the worker
        // so that the last `Arc` usually drops there. Otherwise jni auto-attaches this thread
        // and logs a warning. If the worker's clone drops first, this thread pays that cost.
        // In both cases the reference is deleted once.
        let object = self.0.clone();
        let _ = crate::jvm::inbox::submit(crate::jvm::inbox::Cmd::ReleaseRef { object });
    }
}

/// Classifies one value crossing Clojure → Lisp:
/// - `null` → [`LispInput::Scalar`]`(Nil)`;
/// - a `LispObject` → [`LispInput::Obj`]`(ptr)`;
/// - a `LispSymbol` → [`LispInput::Symbol`];
/// - anything else → [`LispInput::Scalar`], via [`to_bridge`].
pub(crate) fn to_input<'local>(
    env: &mut jni::Env<'local>,
    elem: JObject<'local>,
) -> jni::errors::Result<LispInput> {
    // JNI `IsInstanceOf` is true for null against any class, so check null first.
    if elem.is_null() {
        return Ok(LispInput::Scalar(BridgeValue::Nil));
    }
    let lisp_object_class = env.find_class(jni::jni_str!("emacs_jni/core/LispObject"))?;
    if env.is_instance_of(&elem, &lisp_object_class)? {
        return Ok(LispInput::Obj(crate::jvm::entry::get_ptr_field(env, &elem)?));
    }
    let lisp_symbol_class = env.find_class(jni::jni_str!("emacs_jni/core/LispSymbol"))?;
    if env.is_instance_of(&elem, &lisp_symbol_class)? {
        let name_obj =
            env.get_field(&elem, jni::jni_str!("name"), jni::jni_sig!(java.lang.String))?.l()?;
        let name = env.cast_local::<JString>(name_obj)?;
        let name = name.try_to_string(env)?;
        let keyword =
            env.get_field(&elem, jni::jni_str!("keyword"), jni::jni_sig!(boolean))?.z()?;
        return Ok(LispInput::Symbol { name, keyword });
    }
    let lisp_arg = to_bridge(env, JValueOwned::Object(elem))?;
    Ok(LispInput::Scalar(lisp_arg))
}

/// Converts the `Object[]` `args` of `applyAsync` to [`LispInput`]s.
pub(crate) fn jobject_array_to_lisp_request_args<'local>(
    env: &mut jni::Env<'local>,
    args: &JObjectArray<'local>,
) -> jni::errors::Result<Vec<LispInput>> {
    let len = args.len(env)?;
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let elem = args.get_element(env, i)?;
        out.push(to_input(env, elem)?);
    }
    Ok(out)
}

/// Converts the `Object[]` arguments of a Java call to a Lisp function to [`BridgeValue`]s.
pub(crate) fn jobject_array_to_bridge_values(
    env: &mut jni::Env<'_>,
    args: &JObjectArray<'_>,
) -> jni::errors::Result<Vec<BridgeValue>> {
    let len = args.len(env)?;
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let obj = args.get_element(env, i)?;
        out.push(to_bridge(env, JValueOwned::Object(obj))?);
    }
    Ok(out)
}

#[cfg(test)]
mod jvm_tests {
    use super::*;
    use crate::jvm::test_support::create_test_jvm;

    #[test]
    fn int_arg_widens_correctly_to_a_long_parameter() {
        let vm = create_test_jvm();
        vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            // `Long.valueOf(long)` has a primitive `long` parameter.
            let long_class = env.find_class(jni::jni_str!("java/lang/Long"))?;
            let method_name = env.new_string("valueOf")?;
            let class_class = env.find_class(jni::jni_str!("java/lang/Class"))?;
            let long_primitive = primitive_long_class(env)?;
            let param_types = env.new_object_array(1, &class_class, &long_primitive)?;
            let method_name_arg = JValueOwned::Object(method_name.into());
            let param_types_arg = JValueOwned::Object(param_types.into());
            let methods = env
                .call_method(
                    &long_class,
                    jni::jni_str!("getMethod"),
                    jni::jni_sig!((name: java.lang.String, paramTypes: [java.lang.Class]) -> java.lang.reflect.Method),
                    &[method_name_arg.borrow(), param_types_arg.borrow()],
                )?
                .l()?;
            let method = env.cast_local::<jni::objects::JObject>(methods)?;

            let long_primitive = primitive_long_class(env)?;
            let arg = from_bridge(env, &BridgeValue::Int(42), &long_primitive)?;
            let object_class = env.find_class(jni::jni_str!("java/lang/Object"))?;
            let args_array = env.new_object_array(1, &object_class, JObject::null())?;
            let boxed_arg = box_if_primitive(env, arg)?;
            args_array.set_element(env, 0, boxed_arg)?;

            let receiver_arg = JValueOwned::Object(JObject::null());
            let args_array_arg = JValueOwned::Object(args_array.into());
            let result = env
                .call_method(
                    &method,
                    jni::jni_str!("invoke"),
                    jni::jni_sig!((obj: java.lang.Object, args: [java.lang.Object]) -> java.lang.Object),
                    &[receiver_arg.borrow(), args_array_arg.borrow()],
                )?
                .l()?;
            let result_arg = object_to_bridge(env, result)?;
            // A `Long` misrouted to `Float` would also print as 42, so match the variant.
            assert!(
                matches!(result_arg, BridgeValue::Int(42)),
                "expected BridgeValue::Int(42), got {result_arg:?}"
            );
            Ok(())
        })
        .expect("int-to-long marshalling round trip should succeed");
    }

    fn primitive_long_class<'local>(
        env: &mut jni::Env<'local>,
    ) -> jni::errors::Result<JClass<'local>> {
        let long_class = env.find_class(jni::jni_str!("java/lang/Long"))?;
        let field = env
            .get_static_field(&long_class, jni::jni_str!("TYPE"), jni::jni_sig!(java.lang.Class))?
            .l()?;
        env.cast_local::<JClass>(field)
    }
}
