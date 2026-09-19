use emacs::{Env, IntoLisp, Result as EmacsResult, Value};

use crate::jvm::convert::JvmHandle;
use crate::protocol::{BridgeValue, LispInput, LispOutcome};

/// Converts a single Lisp value to a `BridgeValue` by runtime type, trying
/// each marshallable Lisp type in turn.
pub fn to_bridge(v: Value) -> EmacsResult<BridgeValue> {
    if !v.is_not_nil() {
        return Ok(BridgeValue::Nil);
    }
    if v.env.call("keywordp", (v,))?.is_not_nil() {
        let name: String = v.env.call("symbol-name", (v,))?.into_rust()?;
        // `symbol-name` of a keyword includes its leading `:`; strip it so
        // `name` is the bare name the marker carries.
        let name = name.strip_prefix(':').unwrap_or(&name).to_string();
        return Ok(BridgeValue::Symbol { name, keyword: true });
    }
    if let Ok(n) = v.into_rust::<i64>() {
        return Ok(BridgeValue::Int(n));
    }
    // A Lisp integer (fixnum or bignum) that didn't fit in i64 above is a
    // bignum too large for a 64-bit register — this must be routed
    // through `BridgeValue::BigInt` via its exact decimal string, not the
    // `f64` attempt below. `extract_float` (which backs `into_rust::<f64>`)
    // coerces any Lisp number, including bignums, so skipping this check
    // would silently accept a bignum as a lossy `BridgeValue::Float` instead of
    // erroring or round-tripping exactly — the same "don't silently
    // misrepresent" concern `jvm::convert::to_bridge` (Task 4)
    // already applies on the Java-to-Lisp direction for boxed Long/Integer.
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
    // `t` has no useful into_rust::<T>() scalar conversion (there's no
    // `FromLisp` impl for `bool` — Emacs Lisp has no dedicated boolean
    // type), so it's checked explicitly here via `eq` against the
    // interned `t` symbol, matching Lisp's own notion of "is this
    // literally `t`" rather than "is this any non-nil symbol".
    if v == v.env.intern("t")? {
        return Ok(BridgeValue::True);
    }
    // A symbol is data: it crosses as a marker, not a callback. Checked
    // before `functionp` so a symbol holding a function cell (`car`) is a
    // marker too; only a non-symbol function object (lambda, closure,
    // byte-code, subr) reaches the `functionp` arm below. Matches the
    // Clojure `->lisp` model, where every symbol is a `LispSymbol` marker
    // and callability is opt-in via `require-symbol`.
    if v.env.call("symbolp", (v,))?.is_not_nil() {
        let name: String = v.env.call("symbol-name", (v,))?.into_rust()?;
        return Ok(BridgeValue::Symbol { name, keyword: false });
    }
    // A Lisp function (symbol with a function cell, lambda, closure,
    // subr, or byte-compiled object) is registered Lisp-side in
    // `jvm--lisp-callbacks' rather than tracked here — this module never
    // holds a reference to the function itself, only its handle. See
    // spec "Identity and lifetime": Lisp owns that table, since Rust has
    // no stable way to hold onto an arbitrary Lisp function value across
    // calls. `jvm::convert::from_bridge` later turns this handle into
    // a real `LispFunction' proxy object when the argument actually
    // crosses into Java.
    //
    // The `emacs` crate (0.21.0, checked against its vendored source —
    // `env.rs`/`value.rs`/`call.rs`) has no `Value::is_function` or
    // similar native predicate, so this calls Lisp's own `functionp'
    // via `env.call`, the same "call back into Lisp for a predicate the
    // crate doesn't expose" pattern already used above for `integerp'.
    if v.env.call("functionp", (v,))?.is_not_nil() {
        let handle: u64 = v.env.call("jvm--register-callback", (v,))?.into_rust()?;
        return Ok(BridgeValue::Callback(handle));
    }
    // Anything else reaching this point (a vector, hash-table, or other
    // unsupported value) is not a value this bridge knows how to marshal.
    // Earlier code silently treated it as `BridgeValue::True`, which could mask
    // real caller bugs (e.g. accidentally passing a vector where a number
    // was intended would silently become Java `true`) -- signal a clear
    // error instead.
    v.env.signal(crate::jvm_invalid_argument, (v,))
}

/// Converts a marshalled `BridgeValue` to a real Lisp `Value` on the main thread.
///
/// Shared by the Lisp-access apply path (`from_input`'s `Scalar` arm), the
/// synchronous `jvm--call-direct` return path (`lib.rs`, which turns the
/// `BridgeValue` `jvm_thread::call_direct` produced into the value the defun
/// returns), and the async `CallDone`/`Invoke` inbound arms
/// (`lisp_requests::service_inbound`'s `handle_message`) — every JVM-to-Lisp
/// value crossing goes through this one converter now, on the main thread,
/// instead of a wire round trip.
pub fn from_bridge<'e>(env: &'e Env, arg: &BridgeValue) -> EmacsResult<Value<'e>> {
    match arg {
        BridgeValue::Nil => ().into_lisp(env),
        BridgeValue::True => true.into_lisp(env),
        BridgeValue::Int(n) => n.into_lisp(env),
        BridgeValue::Float(f) => f.into_lisp(env),
        // Emacs's own reader parses an arbitrary-precision decimal
        // string into a bignum when it doesn't fit a fixnum — reusing
        // it here avoids reimplementing bignum construction.
        BridgeValue::BigInt(digits) => env.call("string-to-number", (digits.as_str(),)),
        BridgeValue::Str(s) => s.as_str().into_lisp(env),
        // Build the `JvmHandle` `user-ptr` directly from the shared
        // reference — no wire, no table, no `jvm--wrap-handle` round trip.
        BridgeValue::Object(object) => Box::new(JvmHandle(object.clone())).into_lisp(env),
        // Intern on the main thread, mirroring the `Intern` request
        // (`from_input`): a keyword takes its leading `:`, a plain symbol
        // its bare name. `env.intern` uses the global obarray, so the result
        // is `eq`-stable.
        BridgeValue::Symbol { name, keyword } => {
            if *keyword {
                env.intern(&format!(":{name}"))
            } else {
                env.intern(name)
            }
        }
        BridgeValue::Callback(_) => unreachable!(
            "neither a Lisp-access apply argument nor a JVM-call return value is ever \
             classified as BridgeValue::Callback -- Callback is only produced on the \
             Lisp-to-Java argument direction (see lisp::convert::to_bridge)"
        ),
    }
}

/// Converts one already-classified apply argument to a real `Value`,
/// mirroring spec "Argument conversion" for the main-thread side.
pub(crate) fn from_input<'e>(env: &'e Env, arg: &LispInput) -> EmacsResult<Value<'e>> {
    match arg {
        LispInput::Obj(ptr) => Ok(crate::lisp_requests::borrow(env, *ptr)),
        // A keyword interns with its leading `:`; a plain symbol by name.
        // `env.intern` uses the global obarray, so the result round-trips
        // `eq`-stably back to a Clojure symbol via `to_outcome`.
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

/// Classifies an `apply` result: a scalar or string converts to the
/// matching Clojure value directly; everything else (symbols, conses,
/// vectors, hash tables, ...) stays an opaque `LispObject`. `nil` becomes
/// Clojure `nil` and the symbol `t` becomes `true`, so a Lisp predicate's
/// result is usable directly in a Clojure conditional. The scalar set and
/// its Java shapes mirror the `jvm-call` return-marshalling table.
///
/// `pub(crate)` because `lib.rs`'s `jvm--invoke-reply` reuses it too:
/// `jvm--resolve-invoke` (`elisp/jvm.el`) hands it the real `(SYMBOL . DATA)`
/// a failed callback's `condition-case` caught, and each half needs exactly
/// this same "classify a raw Lisp value" treatment before it can go into a
/// `LispOutcome::Signal`.
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
        // A fixnum fits an i64; a bignum does not, so render it to its
        // decimal string and rebuild it as a BigInteger on the JVM side.
        if env.call("fixnump", (value,))?.is_not_nil() {
            return Ok(LispOutcome::Value(BridgeValue::Int(value.into_rust::<i64>()?)));
        }
        let digits = env.call("number-to-string", (value,))?.into_rust::<String>()?;
        return Ok(LispOutcome::Value(BridgeValue::BigInt(digits)));
    }
    // A keyword's `symbol-name` includes the leading `:`; carry the bare
    // name. `keywordp` in Emacs already implies interned, so every keyword
    // converts.
    if env.call("keywordp", (value,))?.is_not_nil() {
        let full = env.call("symbol-name", (value,))?.into_rust::<String>()?;
        let name = full.strip_prefix(':').unwrap_or(&full).to_string();
        return Ok(LispOutcome::Value(BridgeValue::Symbol { name, keyword: true }));
    }
    if env.call("symbolp", (value,))?.is_not_nil() {
        // Only an interned symbol (one `eq` to `intern-soft` of its own
        // name, i.e. present in the global obarray) converts to a Clojure
        // symbol. An uninterned symbol (`make-symbol`/`gensym`) stays an
        // opaque `LispObject`, preserving its identity across the round
        // trip — see spec "Result conversion".
        let name = env.call("symbol-name", (value,))?;
        let interned = env.call("intern-soft", (name,))?;
        if env.call("eq", (value, interned))?.is_not_nil() {
            return Ok(LispOutcome::Value(BridgeValue::Symbol {
                name: name.into_rust::<String>()?,
                keyword: false,
            }));
        }
    }
    Ok(LispOutcome::Ptr(crate::lisp_requests::box_value(value)))
}
