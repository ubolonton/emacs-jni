use jni::objects::{JClass, JObject, JValueOwned};

use crate::commands::LispArg;
use crate::handles;

/// Converts ARG to a JNI value matching PARAM_TYPE's actual declared type
/// — see spec "Value marshalling": this is deliberately type-directed,
/// not a fixed independent mapping, because `Method.invoke` only allows
/// widening primitive conversions.
pub fn lisp_arg_to_jvalue<'local>(
    env: &mut jni::Env<'local>,
    arg: &LispArg,
    param_type: &JClass<'local>,
) -> jni::errors::Result<JValueOwned<'local>> {
    let param_name = class_name(env, param_type)?;

    match (arg, param_name.as_str()) {
        (LispArg::Nil, "boolean") | (LispArg::Nil, "java.lang.Boolean") => {
            Ok(JValueOwned::Bool(false))
        }
        (LispArg::Nil, _) => Ok(JValueOwned::Object(JObject::null())),
        (LispArg::True, _) => Ok(JValueOwned::Bool(true)),
        (LispArg::Int(n), "int") => i32::try_from(*n)
            .map(JValueOwned::Int)
            .map_err(|_| marshal_error(&format!("integer {n} out of range for parameter type int"))),
        (LispArg::Int(n), "short") => i16::try_from(*n)
            .map(JValueOwned::Short)
            .map_err(|_| marshal_error(&format!("integer {n} out of range for parameter type short"))),
        (LispArg::Int(n), "byte") => i8::try_from(*n)
            .map(JValueOwned::Byte)
            .map_err(|_| marshal_error(&format!("integer {n} out of range for parameter type byte"))),
        // The "java.lang.Object" arms below matter beyond Task 5's own
        // `Method.invoke`/`Constructor.newInstance` callers (where a
        // declared parameter type of plain `Object` is just one possible
        // overload among others): Task 7's `handle_invoke_reply` always
        // marshals a Lisp callback's return value against exactly
        // `java.lang.Object`, since that's `CompletableFuture<Object>
        // #complete`'s own declared parameter type — with no arm for it, a
        // Lisp function simply returning an integer or float back to Java
        // (the common case) would always fail marshalling here rather than
        // completing the future at all. `long`/`double`/`BigInteger` are
        // the same choices `object_to_lisp_arg` (the reverse direction,
        // below) already treats as canonical for "an arbitrary boxed
        // number", so a value that crosses Java -> Lisp -> Java this way
        // round-trips through the same representation rather than
        // silently narrowing.
        (LispArg::Int(n), "long") | (LispArg::Int(n), "java.lang.Object") => Ok(JValueOwned::Long(*n)),
        (LispArg::Int(n), "float") => Ok(JValueOwned::Float(*n as f32)),
        (LispArg::Int(n), "double") => Ok(JValueOwned::Double(*n as f64)),
        (LispArg::Int(n), "java.math.BigInteger") => big_integer_from_str(env, &n.to_string()),
        (LispArg::Int(_), other) => Err(marshal_error(&format!(
            "integer argument doesn't fit parameter type {other}"
        ))),
        (LispArg::Float(f), "float") => Ok(JValueOwned::Float(*f as f32)),
        (LispArg::Float(f), "double") | (LispArg::Float(f), "java.lang.Object") => Ok(JValueOwned::Double(*f)),
        (LispArg::Float(_), other) => Err(marshal_error(&format!(
            "float argument doesn't fit parameter type {other}"
        ))),
        (LispArg::BigInt(digits), "java.math.BigInteger") | (LispArg::BigInt(digits), "java.lang.Object") => {
            big_integer_from_str(env, digits)
        }
        (LispArg::BigInt(_), other) => Err(marshal_error(&format!(
            "bignum argument too large for parameter type {other}"
        ))),
        (LispArg::Str(s), _) => {
            let jstr = env.new_string(s)?;
            Ok(JValueOwned::Object(jstr.into()))
        }
        // `handles::resolve` returns `Option<Arc<Global<JObject<'static>>>>`
        // (Task 3's corrected signature, verified against the committed
        // `src/handles.rs` — jni 0.22.4's `Global` has no `Clone` impl, so
        // the table stores `Arc`-wrapped `Global`s instead). `Arc` derefs
        // to `Global<JObject<'static>>` the same way a bare `Global` would,
        // so `.as_obj()` works unchanged.
        (LispArg::Object(handle), _) => match handles::resolve(*handle) {
            Some(global) => {
                let local = env.new_local_ref(global.as_obj())?;
                Ok(JValueOwned::Object(local))
            }
            None => Err(marshal_error(&format!("unknown JVM object handle {handle}"))),
        },
        // The handle here already denotes a live entry in Lisp's own
        // `jvm--lisp-callbacks' table (`lisp_value_to_arg` in `lib.rs`
        // registered it there before this `LispArg` was ever built —
        // this module has no access to that table itself, only the
        // handle number). Wrapping it as a `LispFunction' here is what
        // makes "a Lisp function crosses into Java as a real object" a
        // reality: `LispFunction`'s single constructor just stores the
        // handle (see `java/emacs_jni/LispFunction.java`), for later
        // `invoke`/`InvokeReply` round-tripping (Task 7) to call back
        // into Lisp by that handle.
        (LispArg::Callback(handle), _) => {
            let lisp_function_class = env.find_class(jni::jni_str!("emacs_jni/LispFunction"))?;
            let obj = env.new_object(
                &lisp_function_class,
                jni::jni_sig!((handle: long) -> void),
                &[JValueOwned::Long(*handle as i64).borrow()],
            )?;
            Ok(JValueOwned::Object(obj))
        }
    }
}

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

/// Returns PARAM_TYPE's Java-source-style name (e.g. `"int"`,
/// `"java.lang.String"`), by calling `Class.getName()` and translating the
/// internal `/`-separated form JNI normally deals in — reflection's own
/// `Class.getName()` already returns dotted form for reference types and
/// the primitive keyword for primitive types, so no extra translation is
/// actually needed; this just calls it and converts the resulting
/// `JString` to a Rust `String`.
fn class_name(env: &mut jni::Env<'_>, class: &JClass<'_>) -> jni::errors::Result<String> {
    let name = env
        .call_method(class, jni::jni_str!("getName"), jni::jni_sig!(() -> java.lang.String), &[])?
        .l()?;
    let name = env.cast_local::<jni::objects::JString>(name)?;
    Ok(name.to_string())
}

/// Builds a "not a JNI failure, just our own application error" value.
/// `jni::errors::Error` (verified against the vendored 0.22.4 crate
/// source, `src/errors.rs`) has no `From<std::io::Error>` impl and no
/// generic "custom message" variant — it's a closed, non-`#[non_exhaustive]`-
/// for-this-purpose enum of specific JNI/JVM failure shapes. `ParseFailed
/// (String)` is the only variant that (a) carries an arbitrary owned
/// `String` message and (b) isn't tied to a specific unrelated JNI
/// operation's semantics (unlike e.g. `NullPtr(&'static str)`, which also
/// can't take a formatted owned string). Reusing it here — rather than
/// introducing a separate local error type, which would mean every
/// function in this module returning something other than the
/// `jni::errors::Result` the task's documented interface specifies — is a
/// label mismatch (these aren't parse failures), but it's the closest fit
/// available without widening this task's scope into inventing new error
/// infrastructure.
// `pub(crate)` rather than private: Task 5's `jvm_thread.rs` needs the exact
// same "wrap an owned message as a jni::errors::Error" behavior for its own
// application-level errors (unknown handle, no matching overload, etc.) —
// none of those are marshalling failures either, so the name is already a
// bit of a misnomer even here, but introducing a second, identically-bodied
// helper in `jvm_thread.rs` would just be duplication for duplication's
// sake. Reusing this one directly keeps there being exactly one "make an
// ad hoc jni::errors::Error" function in the crate.
pub(crate) fn marshal_error(msg: &str) -> jni::errors::Error {
    jni::errors::Error::ParseFailed(msg.to_string())
}

/// `Method.invoke`/`Constructor.newInstance` always want an `Object[]`,
/// even for parameters whose declared type is primitive — so any
/// primitive `JValueOwned` `lisp_arg_to_jvalue` produced must be boxed
/// before going into that array. Object values (including `null`) pass
/// through unchanged.
pub fn box_if_primitive<'local>(
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

/// Calls `<CLASS_NAME>.valueOf(value)` to box a single primitive
/// `JValueOwned` into its wrapper-class `JObject`. Generic over the
/// signature type `S` (rather than a concrete `&str`, which is not what
/// `jni::jni_sig!` actually expands to — verified against the compiler)
/// so that each call site can just forward whatever `jni::jni_sig!`
/// produces, matching `Env::call_static_method`'s own bound.
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

/// Converts a JNI return value back to a `LispArg`. Non-scalar returns
/// (any Java object that isn't null/Boolean/a numeric wrapper/String/
/// BigInteger) get wrapped via `handles::register` — this is the
/// mechanism behind "wrap a JVM object as a Lisp value" whenever it
/// happens as a side effect of a method's return value, not just via an
/// explicit call.
pub fn jvm_value_to_lisp_arg<'local>(
    env: &mut jni::Env<'local>,
    value: JValueOwned<'local>,
) -> jni::errors::Result<LispArg> {
    match value {
        JValueOwned::Void => Ok(LispArg::Nil),
        JValueOwned::Bool(b) => Ok(if b { LispArg::True } else { LispArg::Nil }),
        JValueOwned::Byte(n) => Ok(LispArg::Int(n as i64)),
        JValueOwned::Short(n) => Ok(LispArg::Int(n as i64)),
        JValueOwned::Int(n) => Ok(LispArg::Int(n as i64)),
        JValueOwned::Long(n) => Ok(LispArg::Int(n)),
        JValueOwned::Float(f) => Ok(LispArg::Float(f as f64)),
        JValueOwned::Double(f) => Ok(LispArg::Float(f)),
        JValueOwned::Char(c) => Ok(LispArg::Int(c as i64)),
        JValueOwned::Object(obj) => object_to_lisp_arg(env, obj),
    }
}

fn object_to_lisp_arg<'local>(
    env: &mut jni::Env<'local>,
    obj: JObject<'local>,
) -> jni::errors::Result<LispArg> {
    if obj.is_null() {
        return Ok(LispArg::Nil);
    }

    let string_class = env.find_class(jni::jni_str!("java/lang/String"))?;
    let boolean_class = env.find_class(jni::jni_str!("java/lang/Boolean"))?;
    let big_integer_class = env.find_class(jni::jni_str!("java/math/BigInteger"))?;
    // Boxed integral wrapper types must be checked (and routed through
    // `longValue()` into `LispArg::Int`) *before* the generic `Number`
    // fallback below — `Method.invoke` auto-boxes every primitive return
    // value into one of these, so this is the dominant numeric-return
    // path, not a corner case. Routing them through the generic
    // `Number`/`doubleValue()`/`LispArg::Float` fallback instead would
    // silently misrepresent the value's type (and lose precision for
    // `Long`s beyond 2^53).
    let integer_class = env.find_class(jni::jni_str!("java/lang/Integer"))?;
    let long_class = env.find_class(jni::jni_str!("java/lang/Long"))?;
    let short_class = env.find_class(jni::jni_str!("java/lang/Short"))?;
    let byte_class = env.find_class(jni::jni_str!("java/lang/Byte"))?;
    let number_class = env.find_class(jni::jni_str!("java/lang/Number"))?;

    if env.is_instance_of(&obj, &string_class)? {
        let s = env.cast_local::<jni::objects::JString>(obj)?;
        return Ok(LispArg::Str(s.to_string()));
    }
    if env.is_instance_of(&obj, &boolean_class)? {
        let b = env
            .call_method(&obj, jni::jni_str!("booleanValue"), jni::jni_sig!(() -> boolean), &[])?
            .z()?;
        return Ok(if b { LispArg::True } else { LispArg::Nil });
    }
    if env.is_instance_of(&obj, &big_integer_class)? {
        let s = env
            .call_method(&obj, jni::jni_str!("toString"), jni::jni_sig!(() -> java.lang.String), &[])?
            .l()?;
        let s = env.cast_local::<jni::objects::JString>(s)?;
        return Ok(LispArg::BigInt(s.to_string()));
    }
    if env.is_instance_of(&obj, &integer_class)?
        || env.is_instance_of(&obj, &long_class)?
        || env.is_instance_of(&obj, &short_class)?
        || env.is_instance_of(&obj, &byte_class)?
    {
        let n = env
            .call_method(&obj, jni::jni_str!("longValue"), jni::jni_sig!(() -> long), &[])?
            .j()?;
        return Ok(LispArg::Int(n));
    }
    if env.is_instance_of(&obj, &number_class)? {
        let d = env
            .call_method(&obj, jni::jni_str!("doubleValue"), jni::jni_sig!(() -> double), &[])?
            .d()?;
        return Ok(LispArg::Float(d));
    }

    let global = env.new_global_ref(&obj)?;
    let handle = handles::register(global);
    Ok(LispArg::Object(handle))
}

/// Renders ARG as the S-expression fragment embedded in `invoke`/
/// `call-done` messages (see `protocol::invoke_message`/`call_done_message`).
/// A wrapped JVM object handle round-trips as `(jvm-handle N)` — Task 6's
/// Elisp side reads this tag and turns it back into a real `user-ptr` via
/// a new primitive that wraps a raw handle integer.
pub fn lisp_arg_to_sexp(arg: &LispArg) -> String {
    match arg {
        LispArg::Nil => "nil".to_string(),
        LispArg::True => "t".to_string(),
        LispArg::Int(n) => n.to_string(),
        LispArg::Float(f) => format!("{f}"),
        LispArg::BigInt(digits) => digits.clone(),
        LispArg::Str(s) => quote_lisp_string(s),
        LispArg::Object(handle) => format!("(jvm-handle {handle})"),
        LispArg::Callback(handle) => format!("(jvm-handle {handle})"),
    }
}

/// Same escaping rules as `protocol::quote` (this module doesn't reuse
/// that function directly since it's private to `protocol.rs` — if it
/// turns out both modules need identical logic, consider making
/// `protocol::quote` `pub(crate)` instead of duplicating; for now this
/// mirrors it exactly to keep the two modules independently testable).
fn quote_lisp_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_nil_and_t() {
        assert_eq!(lisp_arg_to_sexp(&LispArg::Nil), "nil");
        assert_eq!(lisp_arg_to_sexp(&LispArg::True), "t");
    }

    #[test]
    fn renders_int_and_float() {
        assert_eq!(lisp_arg_to_sexp(&LispArg::Int(42)), "42");
        assert_eq!(lisp_arg_to_sexp(&LispArg::Float(1.5)), "1.5");
    }

    #[test]
    fn renders_bigint_as_raw_digits() {
        assert_eq!(
            lisp_arg_to_sexp(&LispArg::BigInt("123456789012345678901234567890".to_string())),
            "123456789012345678901234567890"
        );
    }

    #[test]
    fn renders_string_quoted_and_escaped() {
        assert_eq!(
            lisp_arg_to_sexp(&LispArg::Str("hi \"there\"".to_string())),
            "\"hi \\\"there\\\"\""
        );
    }

    #[test]
    fn renders_object_handle() {
        assert_eq!(lisp_arg_to_sexp(&LispArg::Object(7)), "(jvm-handle 7)");
    }
}

#[cfg(test)]
mod jvm_tests {
    use super::*;
    use jni::vm::{InitArgsBuilder, JavaVM};
    use jni::JNIVersion;

    /// jni-rs 0.22.4's `JavaVM::new` does a racy check-then-create against
    /// its process-global singleton (see `src/handles.rs`'s
    /// `create_test_jvm` for the full explanation) -- mirrored here rather
    /// than shared, per Task 3's report noting a shared helper is a
    /// candidate for later cleanup, not required now.
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
    fn int_arg_widens_correctly_to_a_long_parameter() {
        let vm = create_test_jvm();
        vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            // java.lang.Long.valueOf(long) — a real reflective call whose
            // single parameter is declared `long`, proving Int(_) marshals
            // to a JValueOwned::Long that Method.invoke actually accepts.
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
            let arg = lisp_arg_to_jvalue(env, &LispArg::Int(42), &long_primitive)?;
            let object_class = env.find_class(jni::jni_str!("java/lang/Object"))?;
            let args_array = env.new_object_array(1, &object_class, JObject::null())?;
            let boxed_arg = arg_to_object(env, arg)?;
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
            let result_arg = object_to_lisp_arg(env, result)?;
            // Checks the actual variant, not just the rendered string:
            // `lisp_arg_to_sexp`'s `Float` arm renders a whole-number
            // `f64` without a trailing `.0` (Rust's `Display` for `f64`),
            // so `LispArg::Int(42)` and `LispArg::Float(42.0)` render
            // identically as `"42"` — a string-only assertion here
            // couldn't have caught a boxed `Long` return being
            // misrouted into `LispArg::Float` instead of `LispArg::Int`.
            assert!(
                matches!(result_arg, LispArg::Int(42)),
                "expected LispArg::Int(42), got {result_arg:?}"
            );
            assert_eq!(lisp_arg_to_sexp(&result_arg), "42");
            Ok(())
        })
        .expect("int-to-long marshalling round trip should succeed");
    }

    // Helper the test above needs: JValueOwned::Long isn't itself an
    // Object, but Method.invoke's args array needs boxed Objects — a
    // Long primitive value must be boxed via Long.valueOf before going
    // into the array. Rather than hand-roll that boxing logic separately
    // from lisp_arg_to_jvalue (which intentionally returns unboxed
    // primitives for primitive parameter types, matching what
    // Method.invoke itself expects for a *primitive*-typed formal
    // parameter), this test-only helper exists so the test can exercise
    // the reflective invoke path realistically. This is a real open design
    // question (whether lisp_arg_to_jvalue needs a boxed-Object sibling for
    // Task 6's varargs-style Method.invoke callers) for Task 6 to resolve
    // — not solved here; see plan's Task 5 section, where `box_if_primitive`
    // is later introduced for exactly that purpose.
    fn arg_to_object<'local>(env: &mut jni::Env<'local>, v: JValueOwned<'local>) -> jni::errors::Result<JObject<'local>> {
        match v {
            JValueOwned::Object(o) => Ok(o),
            JValueOwned::Long(n) => {
                let long_class = env.find_class(jni::jni_str!("java/lang/Long"))?;
                env.call_static_method(&long_class, jni::jni_str!("valueOf"), jni::jni_sig!((v: long) -> java.lang.Long), &[JValueOwned::Long(n).borrow()])?.l()
            }
            _ => panic!("test helper only handles the cases this test needs"),
        }
    }

    fn primitive_long_class<'local>(env: &mut jni::Env<'local>) -> jni::errors::Result<JClass<'local>> {
        // java.lang.Long.TYPE is the live Class<Long> representing the
        // primitive `long`, distinct from the boxed Long.class.
        let long_class = env.find_class(jni::jni_str!("java/lang/Long"))?;
        let field = env.get_static_field(&long_class, jni::jni_str!("TYPE"), jni::jni_sig!(java.lang.Class))?.l()?;
        env.cast_local::<JClass>(field)
    }
}
