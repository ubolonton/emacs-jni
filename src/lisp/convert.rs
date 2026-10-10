//! Lisp value conversion, on the Emacs main thread: Lisp [`Value`] ↔ [`BridgeValue`] for
//! `jvm-call` arguments and results, and Lisp [`Value`] ↔ [`LispInput`]/[`LispOutcome`] for
//! Clojure's access to Lisp values.

use emacs::{Env, IntoLisp, Result as EmacsResult, Value};

use crate::jvm::convert::JvmHandle;
use crate::protocol::{BridgeValue, LispInput, LispOutcome};

/// Converts a Lisp value to a [`BridgeValue`] by its runtime type. Signals `jvm-invalid-argument`
/// for a type with no Java form, such as a vector.
pub fn to_bridge(v: Value) -> EmacsResult<BridgeValue> {
    if !v.is_not_nil() {
        return Ok(BridgeValue::Nil);
    }
    if v.env.call("keywordp", (v,))?.is_not_nil() {
        let name: String = v.env.call("symbol-name", (v,))?.into_rust()?;
        let name = name.strip_prefix(':').unwrap_or(&name).to_string();
        return Ok(BridgeValue::Symbol { name, keyword: true });
    }
    if let Ok(n) = v.into_rust::<i64>() {
        return Ok(BridgeValue::Int(n));
    }
    // An integer that didn't fit `i64` is a bignum. Check it before the `f64` arm:
    // `into_rust::<f64>` accepts any Lisp number, so a bignum would silently become a lossy
    // `Float`.
    if v.env.call("integerp", (v,))?.is_not_nil() {
        let digits: String = v.env.call("number-to-string", (v,))?.into_rust()?;
        return Ok(BridgeValue::BigInt(digits));
    }
    if let Ok(f) = v.into_rust::<f64>() {
        return Ok(BridgeValue::Float(f));
    }
    if let Ok(s) = v.into_rust::<String>() {
        return Ok(BridgeValue::Str(s));
    }
    if let Ok(h) = v.into_rust::<&JvmHandle>() {
        return Ok(BridgeValue::Object(h.0.clone()));
    }
    // `emacs` has no `FromLisp` for `bool`, so compare with the `t` symbol directly.
    if v == v.env.intern("t")? {
        return Ok(BridgeValue::True);
    }
    // A symbol is data, not a callback, even if it has a function cell (`car`). This matches
    // Clojure's `->lisp`, where callability is opt-in via `require-symbol`.
    if v.env.call("symbolp", (v,))?.is_not_nil() {
        let name: String = v.env.call("symbol-name", (v,))?.into_rust()?;
        return Ok(BridgeValue::Symbol { name, keyword: false });
    }
    // Lisp owns the function table (`jvm--lisp-callbacks`); Rust only carries the handle. See
    // `docs/explanation/interop-design.md`, "Why a Lisp function becomes a Java object".
    if v.env.call("functionp", (v,))?.is_not_nil() {
        let handle: u64 = v.env.call("jvm--register-callback", (v,))?.into_rust()?;
        return Ok(BridgeValue::Callback(handle));
    }
    // Vectors, hash tables, etc. are not marshallable.
    v.env.signal(crate::lisp::entry::jvm_invalid_argument, (v,))
}

/// Converts a [`BridgeValue`] to a Lisp [`Value`]. Every JVM-to-Lisp value goes through this, on
/// the main thread.
///
/// Panics on [`BridgeValue::Callback`], which flows Lisp to Java only.
pub fn from_bridge<'e>(env: &'e Env, arg: &BridgeValue) -> EmacsResult<Value<'e>> {
    match arg {
        BridgeValue::Nil => ().into_lisp(env),
        BridgeValue::True => true.into_lisp(env),
        BridgeValue::Int(n) => n.into_lisp(env),
        BridgeValue::Float(f) => f.into_lisp(env),
        // `string-to-number` makes a bignum when the value doesn't fit a fixnum.
        BridgeValue::BigInt(digits) => env.call("string-to-number", (digits.as_str(),)),
        BridgeValue::Str(s) => s.as_str().into_lisp(env),
        BridgeValue::Object(object) => Box::new(JvmHandle(object.clone())).into_lisp(env),
        // `env.intern` uses the global obarray, so the result is `eq`-stable.
        BridgeValue::Symbol { name, keyword } => {
            if *keyword {
                env.intern(&format!(":{name}"))
            } else {
                env.intern(name)
            }
        }
        BridgeValue::Callback(_) => unreachable!(
            "BridgeValue::Callback only flows Lisp-to-Java (see lisp::convert::to_bridge)"
        ),
    }
}

/// Converts one [`LispInput`] (an `apply` function or argument, or an operand) to a [`Value`].
pub(crate) fn from_input<'e>(env: &'e Env, arg: &LispInput) -> EmacsResult<Value<'e>> {
    match arg {
        LispInput::Obj(ptr) => Ok(crate::lisp::inbox::borrow(env, *ptr)),
        LispInput::Symbol { name, keyword } => {
            if *keyword {
                env.intern(&format!(":{name}"))
            } else {
                env.intern(name)
            }
        }
        LispInput::Scalar(scalar) => from_bridge(env, scalar),
    }
}

/// Classifies a Lisp value for return to Clojure. Scalars, strings, and interned symbols convert
/// to Clojure values; anything else becomes [`LispOutcome::Ptr`], an opaque `LispObject`. `nil`
/// becomes `nil` and `t` becomes `true`, so a Lisp predicate result works in a Clojure
/// conditional.
pub(crate) fn to_outcome(env: &Env, value: Value) -> EmacsResult<LispOutcome> {
    if !value.is_not_nil() {
        return Ok(LispOutcome::Value(BridgeValue::Nil));
    }
    if env.call("eq", (value, env.intern("t")?))?.is_not_nil() {
        return Ok(LispOutcome::Bool(true));
    }
    if env.call("stringp", (value,))?.is_not_nil() {
        return Ok(LispOutcome::Value(BridgeValue::Str(value.into_rust::<String>()?)));
    }
    if env.call("floatp", (value,))?.is_not_nil() {
        return Ok(LispOutcome::Value(BridgeValue::Float(value.into_rust::<f64>()?)));
    }
    if env.call("integerp", (value,))?.is_not_nil() {
        if env.call("fixnump", (value,))?.is_not_nil() {
            return Ok(LispOutcome::Value(BridgeValue::Int(value.into_rust::<i64>()?)));
        }
        let digits = env.call("number-to-string", (value,))?.into_rust::<String>()?;
        return Ok(LispOutcome::Value(BridgeValue::BigInt(digits)));
    }
    // Keywords are always interned, so every keyword converts.
    if env.call("keywordp", (value,))?.is_not_nil() {
        let full = env.call("symbol-name", (value,))?.into_rust::<String>()?;
        let name = full.strip_prefix(':').unwrap_or(&full).to_string();
        return Ok(LispOutcome::Value(BridgeValue::Symbol { name, keyword: true }));
    }
    if env.call("symbolp", (value,))?.is_not_nil() {
        // An uninterned symbol (`make-symbol`, `gensym`) stays an opaque `LispObject`, so its
        // identity survives the round trip.
        let name = env.call("symbol-name", (value,))?;
        let interned = env.call("intern-soft", (name,))?;
        if env.call("eq", (value, interned))?.is_not_nil() {
            return Ok(LispOutcome::Value(BridgeValue::Symbol {
                name: name.into_rust::<String>()?,
                keyword: false,
            }));
        }
    }
    Ok(LispOutcome::Ptr(crate::lisp::inbox::box_value(value)))
}
