use std::sync::Arc;

use jni::objects::{JClass, JObject, JObjectArray, JString, JValueOwned};
use jni::refs::Global;

use crate::protocol::{BridgeValue, LispInput};

/// Converts ARG to a JNI value matching PARAM_TYPE's actual declared type
/// — see spec "Value marshalling": this is deliberately type-directed,
/// not a fixed independent mapping, because `Method.invoke` only allows
/// widening primitive conversions.
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
        // the same choices `object_to_bridge` (the reverse direction,
        // below) already treats as canonical for "an arbitrary boxed
        // number", so a value that crosses Java -> Lisp -> Java this way
        // round-trips through the same representation rather than
        // silently narrowing.
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
        // The handle here already denotes a live entry in Lisp's own
        // `jvm--lisp-callbacks' table (`lisp_value_to_arg` in `lib.rs`
        // registered it there before this `BridgeValue` was ever built —
        // this module has no access to that table itself, only the
        // handle number). Wrapping it as a `LispFunction' here is what
        // makes "a Lisp function crosses into Java as a real object" a
        // reality: `LispFunction`'s single constructor just stores the
        // handle (see `java/emacs_jni/core/LispFunction.java`), for later
        // `invoke`/`InvokeReply` round-tripping (Task 7) to call back
        // into Lisp by that handle.
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
        // A Lisp symbol/keyword crosses into Java as the platform-loader
        // `LispSymbol` marker (identical construction to
        // `jvm_thread::build_lisp_reply_object`'s `Symbol` arm), so the
        // Clojure layer's `<-lisp` and any other consumer read it the same
        // way regardless of which direction produced it.
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

/// Registers a `java.lang.ref.Cleaner` action on OBJ (a freshly constructed
/// `LispFunction` proxy) that, when OBJ becomes phantom-reachable, sends
/// `(release HANDLE)` on the worker-to-Lisp channel — the JVM-side half of
/// the spec's "Identity and lifetime" GC-notification loop, mirroring
/// `JvmHandle::Drop` on the Rust side. The cleaner runs on a
/// JVM-managed background thread at some point after OBJ is collected, not
/// synchronously — this is standard `Cleaner` behavior, not a bug, and
/// exactly why this task has no dedicated automated test asserting the
/// message actually arrives (forcing GC in a test is inherently flaky).
///
/// Done here in Rust, via reflection, rather than as Java-side `Cleaner`
/// code inside `LispFunction` itself: the `Runnable` action passed to
/// `Cleaner#register` needs its own native callback into Rust, and
/// registering a *second* native method just for that (on top of `call`)
/// would be pure duplication of `register_lisp_function_native`'s dance —
/// simpler to reuse a small dedicated shim class (`ReleaseNotifier`,
/// registered by `shim::define_classes` alongside the other three) and
/// drive its construction/registration from here instead.
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
    // `java.lang.ref.Cleaner`/`Cleaner$Cleanable` can't use `jni_sig!`'s
    // unquoted dotted-path syntax (`java.lang.ref.Cleaner`) -- `ref` is a
    // Rust keyword, so the parser can't accept it as a bare path segment.
    // The quoted string-literal form (also documented by `jni_sig!`)
    // sidesteps that; that form's separator for a nested class is `$`, not
    // `::`, hence `Cleaner$Cleanable` below.
    let cleaner = env
        .call_static_method(
            &cleaner_class,
            jni::jni_str!("create"),
            jni::jni_sig!(() -> "java.lang.ref.Cleaner"),
            &[],
        )?
        .l()?;
    // `JValue::Object` (unlike `JValueOwned::Object`) takes a *borrowed*
    // `&JObject` directly, so both arguments here can reference their
    // owning locals (OBJ, a caller-owned reference; `notifier`, owned
    // above) without needing a fresh `new_local_ref` duplicate of either —
    // sidesteps borrowing `env` mutably twice at once (once to build this
    // very argument list, once inside a nested `new_local_ref` call),
    // which an earlier draft of this function ran into.
    env.call_method(
        &cleaner,
        jni::jni_str!("register"),
        jni::jni_sig!((obj: java.lang.Object, action: java.lang.Runnable) -> "java.lang.ref.Cleaner$Cleanable"),
        &[jni::objects::JValue::Object(obj), jni::objects::JValue::Object(&notifier)],
    )?;
    Ok(())
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
// `pub(crate)` rather than private: `jvm_thread.rs` needs the exact
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
/// primitive `JValueOwned` `from_bridge` produced must be boxed
/// before going into that array. Object values (including `null`) pass
/// through unchanged.
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

/// Converts a JNI return value back to a `BridgeValue`. Non-scalar returns
/// (any Java object that isn't null/Boolean/a numeric wrapper/String/
/// BigInteger) are wrapped as a fresh `Global` reference carried directly
/// (Item 6 — no handle table) — this is the mechanism behind "wrap a JVM
/// object as a Lisp value" whenever it happens as a side effect of a
/// method's return value, not just via an explicit call.
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
    // Boxed integral wrapper types must be checked (and routed through
    // `longValue()` into `BridgeValue::Int`) *before* the generic `Number`
    // fallback below — `Method.invoke` auto-boxes every primitive return
    // value into one of these, so this is the dominant numeric-return
    // path, not a corner case. Routing them through the generic
    // `Number`/`doubleValue()`/`BridgeValue::Float` fallback instead would
    // silently misrepresent the value's type (and lose precision for
    // `Long`s beyond 2^53).
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

/// A live global reference to a JVM object, shared by `Arc` clone.
///
/// Replaces the former `u64` handle plus process-global table. The table
/// only existed so a JVM object could be a text-serialisable integer over
/// the old sexp pipe (deleted in Item 2). `jni::refs::Global` is `Send +
/// Sync` (0.22.4), so the reference itself rides the typed `Cmd`/`Inbound`
/// queues directly. `Arc` because 0.22's `Global` has no `Clone` (a real
/// clone is a fresh `NewGlobalRef`, needing a live `Env`); an `Arc` clone
/// is a cheap refcount bump.
#[derive(Debug, Clone)]
pub struct JvmObject(pub Arc<Global<JObject<'static>>>);

/// The Lisp-visible wrapper for a JVM object — a `user-ptr`. Exactly one
/// wrapper owns a given reference (mirrors `LispObject`'s single-owner
/// rule): each JVM→Lisp crossing mints a fresh `Global`, and the wrapper
/// is neither cloned nor copied.
pub struct JvmHandle(pub JvmObject);

// `Transfer` is the `emacs` crate's marker for "expose as an opaque,
// GC-owned `user-ptr`" (vendored emacs-0.21.0, `src/types/user_ptr.rs`) —
// a bare impl, no required method.
impl emacs::Transfer for JvmHandle {}

impl Drop for JvmHandle {
    fn drop(&mut self) {
        // Lisp's GC may run this on a thread with no JNI attachment, so the
        // reference is cloned and handed to the worker thread to release:
        // in the common case, dropping the last `Arc` there runs
        // `Global::Drop`'s `DeleteGlobalRef` on an attached thread, avoiding
        // `jni::Global::Drop`'s GC-thread auto-attach and its logged perf
        // warning (jni-0.22.4, `src/refs/global.rs`). This is best-effort,
        // not a guarantee: if the worker drops its clone before the field
        // drop below runs, this (GC) thread ends up holding the last `Arc`
        // and pays the auto-attach cost itself. Either way the delete runs
        // exactly once -- never a double-free, never a leak.
        let object = self.0.clone();
        let _ = crate::worker::send_command(crate::commands::Cmd::ReleaseRef { object });
    }
}

/// Classifies one value crossing Clojure → Lisp (an `apply` receiver or
/// argument, or a `typeOf`/`isNotNil`/`eq` operand) into a `LispInput`,
/// per spec "Argument conversion":
/// - `null` → `Nil`;
/// - an `emacs_jni.core.LispObject` → carried by its `ptr`;
/// - an `emacs_jni.core.LispSymbol` marker (a Clojure symbol/keyword the
///   namespace wrapped) → `Symbol { name, keyword }`;
/// - everything else → the same `to_bridge` an
///   ordinary `jvm-call` argument would go through.
pub(crate) fn to_input<'local>(
    env: &mut jni::Env<'local>,
    elem: JObject<'local>,
) -> jni::errors::Result<LispInput> {
    // `IsInstanceOf` (what `is_instance_of` calls) reports `true` for a
    // null reference against ANY class per the JNI spec, so null must be
    // checked before the class checks below -- otherwise a null value
    // would be misread as `Obj(0)`.
    if elem.is_null() {
        return Ok(LispInput::Scalar(BridgeValue::Nil));
    }
    let lisp_object_class = env.find_class(jni::jni_str!("emacs_jni/core/LispObject"))?;
    if env.is_instance_of(&elem, &lisp_object_class)? {
        return Ok(LispInput::Obj(crate::shim::get_ptr_field(env, &elem)?));
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

/// Converts `applyAsync`'s `Object[]` ARGS into `LispInput`s, element
/// by element via `to_input`.
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

/// Renders an `Object[]` of `call`'s actual varargs arguments into a
/// `Vec<BridgeValue>` — the reverse direction of what `jvm_thread::
/// handle_call` (Task 5) does for its own return values, reused here since
/// a JVM-initiated call's arguments need the exact same Java-object ->
/// `BridgeValue` pipeline as any other value crossing from Java to Lisp.
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
    use jni::vm::{InitArgsBuilder, JavaVM};
    use jni::JNIVersion;

    /// jni-rs 0.22.4's `JavaVM::new` does a racy check-then-create against
    /// its process-global singleton, with no synchronization of its own: if
    /// this test's `JNI_CreateJavaVM` call collides with another
    /// JVM-creating test in this crate running concurrently under `cargo
    /// test`'s default parallelism, the loser gets
    /// `Create(JniCall(AlreadyCreated))` even though a JVM now genuinely
    /// exists -- retrying picks up the winner's now-populated
    /// `JavaVM::singleton()` on the next attempt. Mirrored here rather than
    /// shared, per Task 3's report noting a shared helper is a candidate
    /// for later cleanup, not required now.
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
            let arg = from_bridge(env, &BridgeValue::Int(42), &long_primitive)?;
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
            let result_arg = object_to_bridge(env, result)?;
            // Checks the actual variant, not just a stringified value: a
            // boxed `Long` return misrouted into `BridgeValue::Float`
            // instead of `BridgeValue::Int` would still print as `42`, so
            // asserting the enum variant directly is what actually catches
            // that misrouting.
            assert!(
                matches!(result_arg, BridgeValue::Int(42)),
                "expected BridgeValue::Int(42), got {result_arg:?}"
            );
            Ok(())
        })
        .expect("int-to-long marshalling round trip should succeed");
    }

    // Helper the test above needs: JValueOwned::Long isn't itself an
    // Object, but Method.invoke's args array needs boxed Objects — a
    // Long primitive value must be boxed via Long.valueOf before going
    // into the array. Rather than hand-roll that boxing logic separately
    // from from_bridge (which intentionally returns unboxed
    // primitives for primitive parameter types, matching what
    // Method.invoke itself expects for a *primitive*-typed formal
    // parameter), this test-only helper exists so the test can exercise
    // the reflective invoke path realistically. This is a real open design
    // question (whether from_bridge needs a boxed-Object sibling for
    // Task 6's varargs-style Method.invoke callers) for Task 6 to resolve
    // — not solved here; see plan's Task 5 section, where `box_if_primitive`
    // is later introduced for exactly that purpose.
    fn arg_to_object<'local>(
        env: &mut jni::Env<'local>,
        v: JValueOwned<'local>,
    ) -> jni::errors::Result<JObject<'local>> {
        match v {
            JValueOwned::Object(o) => Ok(o),
            JValueOwned::Long(n) => {
                let long_class = env.find_class(jni::jni_str!("java/lang/Long"))?;
                env.call_static_method(
                    &long_class,
                    jni::jni_str!("valueOf"),
                    jni::jni_sig!((v: long) -> java.lang.Long),
                    &[JValueOwned::Long(n).borrow()],
                )?
                .l()
            }
            _ => panic!("test helper only handles the cases this test needs"),
        }
    }

    fn primitive_long_class<'local>(
        env: &mut jni::Env<'local>,
    ) -> jni::errors::Result<JClass<'local>> {
        // java.lang.Long.TYPE is the live Class<Long> representing the
        // primitive `long`, distinct from the boxed Long.class.
        let long_class = env.find_class(jni::jni_str!("java/lang/Long"))?;
        let field = env
            .get_static_field(&long_class, jni::jni_str!("TYPE"), jni::jni_sig!(java.lang.Class))?
            .l()?;
        env.cast_local::<JClass>(field)
    }
}
