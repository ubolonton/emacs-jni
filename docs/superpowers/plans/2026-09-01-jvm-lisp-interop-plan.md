# Bidirectional JVM ↔ Lisp Interop (Phase 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let Lisp call arbitrary JVM methods/constructors reflectively
(`jvm-call`/`jvm-new`), let Lisp values (functions, wrapped JVM objects,
scalars) marshal across the boundary in both directions, and let JVM code
call back into Lisp asynchronously via a small Java shim — all as a fully
async, correlation-ID-based message protocol with no thread ever blocking
inside the interop layer itself.

**Architecture:** Extends Phase 1's worker thread + `mpsc` command channel +
`open_channel` pipe-process protocol. The worker thread becomes a
never-blocking dispatcher: `Cmd::Call` (a Lisp-initiated JVM call) runs on
its own short-lived JNI-attached thread so slow calls don't stall other
commands; `Cmd::InvokeReply` (a reply to a JVM-initiated callback) resolves
a pending `CompletableFuture` inline, cheaply. A small Java shim
(`LispCallable`/`LispFunction`/`LispError`), compiled at build time and
injected into the running JVM via `DefineClass`, is what lets a Lisp
function be passed into Java code as a real object. Two independent handle
tables (Rust owns JVM-object `GlobalRef`s; Lisp owns its own function
table) track cross-runtime references, each side notifying the other on
GC via `release`/`ReleaseRef`.

**Tech Stack:** Rust (`emacs` crate, `jni` crate 0.22.4 — same as Phase 1),
`javac` (JDK 26, already required by the test suite) invoked from `build.rs`
to compile the shim, Emacs Lisp, ERT.

**Spec:** `docs/superpowers/specs/2026-09-01-jvm-lisp-interop-design.md`

## Global Constraints

- No thread, on either side, ever blocks waiting for a specific interop
  reply. Every cross-runtime call is a message tagged with a correlation
  ID; "blocking" is a polling convenience layer built on top, never a
  transport requirement (spec: "Design principle").
- No support for wrapping a Lisp function to satisfy a pre-existing
  synchronous Java interface (`Comparator`, `Runnable`, etc.) — only the
  shim's own `LispCallable` interface, which is async-native
  (`CompletableFuture`-returning) by construction.
- Dispatch is reflection-based (`Class.getMethod`/`getConstructor` +
  `Method.invoke`/`Constructor.newInstance`), never code-generated, so it
  works against any already-loaded class including ones loaded at runtime
  via the existing `jvm-load-jar`.
- Marshalling is type-directed: the target parameter's declared type
  decides how a Lisp argument is boxed, not a fixed independent mapping
  (spec: "Value marshalling"). See that section's table for every case.
- Reuses Phase 1's `(ok . err)` convention for error propagation in both
  directions — no new error shape.
- One channel per direction, tag-differentiated — no physical control/data
  plane split (spec: "Control plane vs. data plane").
- No git worktrees for this repo — every task below commits directly to
  `master`.
- Wherever the high-level `emacs` crate doesn't expose something needed
  (a `user-ptr` finalizer hook, bignum extraction), fall back to the
  lower-level `emacs-module` crate's raw bindings rather than blocking on
  it (spec: "Crate dependency policy").

### Note on API uncertainty in this plan

Three pieces of this plan use `jni`/`emacs` crate APIs this codebase hasn't
exercised yet — `Env::define_class` (or equivalent) for injecting the
compiled shim into the running JVM, native-method registration for
`LispFunction.call`, and the `emacs` crate's mechanism for exposing a Rust
struct as a garbage-collected opaque Lisp value (`user-ptr`). Phase 1's own
plan hit the same kind of uncertainty in a few places (see its Task 1's
`env.provide` fallback and Task 4's "adjust `use` statements to match the
compiler's suggested paths" note) and resolved it by writing the
best-understood call and having the implementer verify against
`cargo doc -p emacs -p jni --open` (both crates are vendored locally once
`cargo build` has run once) or the compiler's own error output, adjusting
names/signatures as needed without changing the underlying approach. Do the
same here — Tasks 1 and 3 call this out explicitly at the point it matters.

---

## Task 1: Java shim — compile, embed, and load into the running JVM

The highest API-uncertainty task in this plan (see "Note on API
uncertainty" above) — budget extra iteration time.

**Files:**
- Create: `java/emacs_jni/LispCallable.java`
- Create: `build.rs`
- Modify: `Cargo.toml` (nothing new to depend on; `build.rs` just needs to
  exist — Cargo picks it up automatically)
- Create: `src/shim.rs`
- Modify: `src/lib.rs` (add `mod shim;`)
- Modify: `.gitignore` (ignore the compiled shim classes directory)

**Interfaces:**
- Produces: `shim::define_classes(env: &mut jni::Env) -> jni::errors::Result<()>`,
  called once from `jvm_thread::create_jvm` (Task 5 wires this in) right
  after the JVM is created and attached. After this returns successfully,
  `env.find_class(jni::jni_str!("emacs_jni/LispFunction"))`,
  `env.find_class(jni::jni_str!("emacs_jni/LispError"))`, and
  `env.find_class(jni::jni_str!("emacs_jni/LispCallable"))` must all
  succeed for the rest of this plan to work — every later task that needs
  these classes just calls `find_class` fresh each call, matching Phase
  1's existing style (no caching layer; see `add_jar_url`'s repeated
  `find_class` calls in `jvm_thread.rs`).

- [ ] **Step 1: Write the shim source**

`java/emacs_jni/LispCallable.java`:
```java
package emacs_jni;

import java.util.concurrent.CompletableFuture;

public interface LispCallable {
    CompletableFuture<Object> call(Object... args);
}
```

Java requires each top-level type in a package to resolve to its own
`.class` file, but only one `public` top-level type is allowed per source
file — `LispFunction` and `LispError` go in their own files, both
package-private is fine since callers only ever reference them via
`LispCallable` (for `LispFunction`) or via `Throwable`/exception handling
(for `LispError`); `LispError` still needs to be `public` because
`completeExceptionally` needs to construct and eventually let calling code
catch it outside the package, so make it public too:

`java/emacs_jni/LispFunction.java`:
```java
package emacs_jni;

import java.util.concurrent.CompletableFuture;

final class LispFunction implements LispCallable {
    private final long handle;

    LispFunction(long handle) {
        this.handle = handle;
    }

    @Override
    public native CompletableFuture<Object> call(Object... args);
}
```

`java/emacs_jni/LispError.java`:
```java
package emacs_jni;

public final class LispError extends RuntimeException {
    public LispError(String message) {
        super(message);
    }
}
```

- [ ] **Step 2: Write `build.rs` to compile the shim to a stable path**

Compiles to `target/emacs-jni-shim-classes/` (not `OUT_DIR`, which is a
per-build hashed path) so both `include_bytes!` (Step 3) and later test
fixtures that need to reference `emacs_jni.LispCallable` on their own
`javac -cp` (Task 9) have one predictable location to point at.

```rust
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
```

- [ ] **Step 3: Run `cargo build` to confirm the shim compiles**

Run: `cargo build`
Expected: succeeds (this only compiles the shim; nothing embeds it yet).
Confirm the class files exist: `ls target/emacs-jni-shim-classes/emacs_jni/`
should show `LispCallable.class`, `LispFunction.class`, `LispError.class`.

- [ ] **Step 4: Write `src/shim.rs`**

Embeds the three compiled classes and defines them into the running JVM,
using the **system classloader** (not null/bootstrap) so that later plain
`env.find_class("emacs_jni/...")` calls — made from native code with no
Java frames on the stack, which resolve via the system classloader by
default — can find them again. `LispCallable` must be defined before
`LispFunction` (which implements it) for the JVM's verifier to resolve the
interface reference; `LispError` has no dependency and can go anywhere.

```rust
use jni::objects::{JClass, JObject};

const LISP_CALLABLE_CLASS: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/target/emacs-jni-shim-classes/emacs_jni/LispCallable.class"));
const LISP_FUNCTION_CLASS: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/target/emacs-jni-shim-classes/emacs_jni/LispFunction.class"));
const LISP_ERROR_CLASS: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/target/emacs-jni-shim-classes/emacs_jni/LispError.class"));

/// Defines the `emacs_jni.{LispCallable,LispFunction,LispError}` shim
/// classes into the currently-attached JVM, using the system classloader so
/// later `find_class` calls (made with no Java frames on the stack) can
/// find them again by name. Must be called once, right after the JVM is
/// created, before anything else in this crate references those classes.
///
/// VERIFY: `Env::define_class` is this plan's best guess at the jni-rs
/// 0.22.4 method name/signature for JNI's `DefineClass`. If it doesn't
/// exist under that name, check `cargo doc -p jni --open` (the crate is
/// vendored locally after `cargo build`) or grep `~/.cargo/registry` for
/// `jni-0.22.4/src` for the actual method — the standard JNI operation
/// this needs is `DefineClass(name, loader, buf)`, so look for whatever
/// jni-rs calls that.
pub fn define_classes(env: &mut jni::Env) -> jni::errors::Result<()> {
    let class_loader_class = env.find_class(jni::jni_str!("java/lang/ClassLoader"))?;
    let system_loader = env
        .call_static_method(
            &class_loader_class,
            jni::jni_str!("getSystemClassLoader"),
            jni::jni_sig!(() -> java.lang.ClassLoader),
            &[],
        )?
        .l()?;

    define_one(env, &system_loader, "emacs_jni/LispCallable", LISP_CALLABLE_CLASS)?;
    define_one(env, &system_loader, "emacs_jni/LispFunction", LISP_FUNCTION_CLASS)?;
    define_one(env, &system_loader, "emacs_jni/LispError", LISP_ERROR_CLASS)?;

    // Sanity check: confirm find_class can see what was just defined, the
    // same way every later task will look these classes up.
    env.find_class(jni::jni_str!("emacs_jni/LispCallable"))?;
    env.find_class(jni::jni_str!("emacs_jni/LispFunction"))?;
    env.find_class(jni::jni_str!("emacs_jni/LispError"))?;

    Ok(())
}

fn define_one<'local>(
    env: &mut jni::Env<'local>,
    loader: &JObject<'_>,
    name: &str,
    bytecode: &[u8],
) -> jni::errors::Result<JClass<'local>> {
    // VERIFY: exact method name/signature against jni-rs 0.22.4's actual
    // API (see doc comment on `define_classes` above).
    env.define_class(name, loader, bytecode)
}
```

- [ ] **Step 5: Wire up the module**

In `src/lib.rs`, add near the other `mod` declarations:
```rust
mod shim;
```

- [ ] **Step 6: Write a unit test that actually loads the shim into a real JVM**

Add to `src/shim.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use jni::vm::{InitArgsBuilder, JavaVM};
    use jni::JNIVersion;

    fn create_test_jvm() -> JavaVM {
        let init_args = InitArgsBuilder::new().version(JNIVersion::V1_8).build().unwrap();
        JavaVM::new(init_args).unwrap()
    }

    #[test]
    fn defines_all_three_shim_classes_and_they_are_findable() {
        let vm = create_test_jvm();
        vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            define_classes(env)?;
            // Confirm LispFunction really implements LispCallable, not
            // just that both classes independently exist.
            let lisp_function_class = env.find_class(jni::jni_str!("emacs_jni/LispFunction"))?;
            let lisp_callable_class = env.find_class(jni::jni_str!("emacs_jni/LispCallable"))?;
            let is_assignable = env.is_assignable_from(&lisp_function_class, &lisp_callable_class)?;
            assert!(is_assignable, "LispFunction should implement LispCallable");
            Ok(())
        })
        .expect("defining shim classes into a fresh JVM should succeed");
    }
}
```

`Env::is_assignable_from` is a guess at the method name too — if it
doesn't exist under that name, an equivalent check is calling
`Class.isAssignableFrom` reflectively via `call_method`, which is
guaranteed to exist since it's plain Java.

- [ ] **Step 7: Run the test**

Run: `cargo test --lib shim::`
Expected: `defines_all_three_shim_classes_and_they_are_findable` passes.
This is the step most likely to need real iteration against compiler
errors — see "Note on API uncertainty" at the top of this plan. Do not
move to Task 2 until this genuinely passes against a real JVM; every later
task assumes these three classes are loadable.

- [ ] **Step 8: Update `.gitignore` and commit**

Add to `.gitignore`:
```
/target/emacs-jni-shim-classes/
```
(the compiled `.class` files are build artifacts, not source — `java/*.java` is the source of truth and is committed normally.)

```bash
git add java/emacs_jni/LispCallable.java java/emacs_jni/LispFunction.java java/emacs_jni/LispError.java build.rs src/shim.rs src/lib.rs .gitignore
git commit -m "Add Java shim (LispCallable/LispFunction/LispError), compiled and DefineClass-injected at JVM start"
```

---

## Task 2: Wire protocol extension (pure Rust)

**Files:**
- Modify: `src/protocol.rs` (add new message formatters + tests)
- Modify: `src/commands.rs` (add new `Cmd` variants)

**Interfaces:**
- Produces: `pub fn call_done_message(id: u64, ok: bool, result: Option<&str>, err: Option<&str>) -> String`,
  `pub fn invoke_message(id: u64, handle: u64, args: &str) -> String`,
  `pub fn release_message(handle: u64) -> String`. New `Cmd` variants:
  `Call { id: u64, target: CallTarget, member: String, args: Vec<LispArg> }`,
  `InvokeReply { id: u64, ok: bool, result: LispArg }`, `ReleaseRef { handle: u64 }`.
  `CallTarget` and `LispArg` are placeholder enums refined by Task 4's
  marshalling work — defined here just enough to keep `Cmd` compiling; Task
  4 is the one that gives them real variants.

**Note on `RESULT`/`ARGS` string encoding**: this task treats marshalled
values as pre-formatted S-expression fragments (plain `&str`/`String`),
matching how `protocol::quote` already works for error messages in Phase
1. Task 4 (marshalling) is responsible for producing those fragments; this
task only defines the message shapes around them.

- [ ] **Step 1: Add new `CallTarget`/`LispArg` placeholder types and `Cmd` variants to `src/commands.rs`**

```rust
#[derive(Debug, Clone)]
pub enum CallTarget {
    /// Fully-qualified class name, for a static call or `member ==
    /// "<init>"` construction.
    ClassName(String),
    /// A previously wrapped JVM object, by its handle in the Rust-side
    /// GlobalRef table (Task 3).
    Object(u64),
}

/// A single marshalled argument or return value, in Rust form (not yet
/// converted to a JNI `JValue` — that conversion happens in Task 4, which
/// is also the task that will likely extend this enum with more variants
/// as marshalling is implemented; keep this open to growth, don't treat
/// this exact variant set as final).
#[derive(Debug, Clone)]
pub enum LispArg {
    Nil,
    True,
    Int(i64),
    Float(f64),
    /// Decimal-digit string form of a bignum too large for `i64` — see
    /// spec "Value marshalling" > "Bignum mechanism".
    BigInt(String),
    Str(String),
    /// A previously wrapped JVM object, by handle (Task 3).
    Object(u64),
    /// A Lisp function, by its handle in the Lisp-side function table
    /// (Task 6) — auto-wrapped as `LispFunction` when this crosses into
    /// Java (Task 6).
    Callback(u64),
}
```

Add to the existing `Cmd` enum (do not remove `Resume`/`Pause`/`Stop`/`LoadJar`):
```rust
#[derive(Debug)]
pub enum Cmd {
    Resume,
    Pause,
    Stop,
    LoadJar { id: u64, path: String },
    Call { id: u64, target: CallTarget, member: String, args: Vec<LispArg> },
    InvokeReply { id: u64, ok: bool, result: LispArg },
    ReleaseRef { handle: u64 },
    #[cfg(feature = "test-support")]
    TestPanic,
}
```

- [ ] **Step 2: Add new message formatters and tests to `src/protocol.rs`**

```rust
pub fn call_done_message(id: u64, ok: bool, result: Option<&str>, err: Option<&str>) -> String {
    let ok_sym = if ok { "t" } else { "nil" };
    let result_sexp = result.unwrap_or("nil");
    let err_sexp = match err {
        Some(e) => quote(e),
        None => "nil".to_string(),
    };
    format!("(call-done {id} {ok_sym} {result_sexp} {err_sexp})\n")
}

/// ARGS is a pre-formatted S-expression list fragment, e.g. `"(1 2)"` or
/// `"nil"` for zero args — produced by Task 4's marshalling code.
pub fn invoke_message(id: u64, handle: u64, args: &str) -> String {
    format!("(invoke {id} {handle} {args})\n")
}

pub fn release_message(handle: u64) -> String {
    format!("(release {handle})\n")
}
```

Add to the existing `#[cfg(test)] mod tests` block in the same file:
```rust
#[test]
fn formats_call_done_success() {
    assert_eq!(
        call_done_message(7, true, Some("42"), None),
        "(call-done 7 t 42 nil)\n"
    );
}

#[test]
fn formats_call_done_failure() {
    assert_eq!(
        call_done_message(7, false, None, Some("boom")),
        "(call-done 7 nil nil \"boom\")\n"
    );
}

#[test]
fn formats_invoke_with_args() {
    assert_eq!(invoke_message(3, 9, "(1 2)"), "(invoke 3 9 (1 2))\n");
}

#[test]
fn formats_invoke_with_no_args() {
    assert_eq!(invoke_message(3, 9, "nil"), "(invoke 3 9 nil)\n");
}

#[test]
fn formats_release() {
    assert_eq!(release_message(9), "(release 9)\n");
}
```

- [ ] **Step 3: Run the unit tests**

Run: `cargo test --lib protocol:: commands::`
Expected: all new tests pass; `cargo build` still succeeds even though
`Cmd::Call`/`InvokeReply`/`ReleaseRef` aren't handled anywhere yet (Rust
allows an enum to have unhandled-elsewhere variants; only a `match`
without a catch-all would fail to compile, and `jvm_thread::run`'s
existing `match` needs a catch-all added — do that now so `cargo build`
stays green):

In `src/jvm_thread.rs`, find the `match cmd { ... }` inside `run` and add
a catch-all arm for the three new variants (Tasks 5, 7, 8 replace this
with real handling, one variant at a time — do not implement them here):
```rust
            Cmd::Call { .. } | Cmd::InvokeReply { .. } | Cmd::ReleaseRef { .. } => {}
```

- [ ] **Step 4: Run the full existing test suite to confirm nothing broke**

Run: `cargo test --lib`
Expected: all tests pass, including Phase 1's existing ones.

- [ ] **Step 5: Commit**

```bash
git add src/protocol.rs src/commands.rs src/jvm_thread.rs
git commit -m "Extend wire protocol with call-done/invoke/release messages and Cmd variants"
```

---

## Task 3: JVM-object handle table (Rust side) and its Lisp-side `user-ptr`

**Files:**
- Create: `src/handles.rs`
- Modify: `src/lib.rs` (add `mod handles;`, register the `user-ptr` type)

**Interfaces:**
- Produces: `pub struct JvmHandle(pub u64)` — the Lisp-visible wrapper for
  a JVM object, exposed as a `user-ptr` (see Step 3's API-uncertainty
  note). `pub fn register(obj: jni::refs::Global<jni::objects::JObject<'static>>) -> u64`,
  `pub fn resolve(handle: u64) -> Option<jni::refs::Global<jni::objects::JObject<'static>>>`,
  `pub fn release(handle: u64)`. Task 4 (marshalling) calls `register`
  when wrapping a Java return value and `resolve` when unwrapping a
  `JvmHandle` argument; `JvmHandle`'s own `Drop` impl calls `release` and
  sends `Cmd::ReleaseRef` (Task 8 extends `jvm_thread::run`'s handling of
  that variant beyond the placeholder from Task 2).

- [ ] **Step 1: Write the handle table with unit tests**

```rust
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use jni::objects::JObject;
use jni::refs::Global;

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);
static TABLE: Mutex<Option<HashMap<u64, Global<JObject<'static>>>>> = Mutex::new(None);

fn table() -> std::sync::MutexGuard<'static, Option<HashMap<u64, Global<JObject<'static>>>>> {
    let mut guard = TABLE.lock().unwrap();
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    guard
}

/// Registers a JVM object's global reference under a fresh handle,
/// returning that handle. The caller (Task 4) is responsible for actually
/// creating the `Global` ref via `env.new_global_ref` before calling this
/// — this function only owns the handle-to-ref mapping, not JNI concerns.
pub fn register(obj: Global<JObject<'static>>) -> u64 {
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::SeqCst);
    table().as_mut().unwrap().insert(handle, obj);
    handle
}

/// Looks up the global ref for a handle, without removing it — callers
/// needing a fresh local ref to pass into a JNI call should call
/// `env.new_local_ref` on the result themselves (the returned `Global` is
/// borrowed from the table's storage via clone, since `Global` derefs to
/// a reference usable in JNI calls but the table must keep ownership).
pub fn resolve(handle: u64) -> Option<Global<JObject<'static>>> {
    table().as_ref().unwrap().get(&handle).cloned()
}

/// Drops the table's entry for `handle`, releasing the underlying
/// `GlobalRef` (via `Global`'s own `Drop` impl, which calls
/// `DeleteGlobalRef` — this function does not itself touch JNI).
pub fn release(handle: u64) {
    table().as_mut().unwrap().remove(&handle);
}

#[cfg(test)]
mod tests {
    use super::*;
    use jni::vm::{InitArgsBuilder, JavaVM};
    use jni::JNIVersion;

    fn create_test_jvm() -> JavaVM {
        let init_args = InitArgsBuilder::new().version(JNIVersion::V1_8).build().unwrap();
        JavaVM::new(init_args).unwrap()
    }

    #[test]
    fn register_then_resolve_returns_the_same_object() {
        let vm = create_test_jvm();
        vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            let obj = env.new_object(
                &env.find_class(jni::jni_str!("java/lang/Object"))?,
                jni::jni_sig!(() -> void),
                &[],
            )?;
            let global = env.new_global_ref(&obj)?;
            let handle = register(global);
            assert!(resolve(handle).is_some());
            release(handle);
            assert!(resolve(handle).is_none());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn resolve_of_unknown_handle_is_none() {
        assert!(resolve(999_999).is_none());
    }
}
```

Note: `Global`'s exact type parameters mirror what Phase 1's `jvm_thread.rs`
already uses (`jni::refs::Global<jni::objects::JObject<'static>>`) — copy
that exact path rather than guessing a different one, since it's proven to
compile in this codebase already.

- [ ] **Step 2: Run the unit tests**

Run: `cargo test --lib handles::`
Expected: both tests pass.

- [ ] **Step 3: Expose `JvmHandle` as a Lisp `user-ptr`**

**VERIFY** this step against the `emacs` crate's actual API before writing
final code — this plan's best understanding is that the crate exposes a
Rust struct as an opaque, garbage-collected Lisp value via a trait
(commonly named `Transfer` in emacs-module-rs-style crates) that, once
implemented for a type, lets `#[defun]`s return that type directly (via
`IntoLisp`) and receive it back as an argument (via `FromLisp`), with
cleanup wired to the type's own `Drop` impl when Lisp's GC collects the
value. Check `cargo doc -p emacs --open` (vendored locally after `cargo
build`) for the actual trait name/bounds; if it needs something more than
a bare `Drop` impl (e.g. an explicit marker trait to opt in), add that.

Add to `src/handles.rs`:
```rust
/// The Lisp-visible wrapper for a JVM object — a `user-ptr` under the
/// hood. Its `Drop` impl is what makes "wrap a JVM object as a Lisp
/// value" actually release the `GlobalRef` when Lisp's GC collects the
/// wrapper (spec: "Identity and lifetime").
pub struct JvmHandle(pub u64);

// VERIFY: confirm the actual trait name/bounds the `emacs` crate wants
// for "expose this Rust struct as a user-ptr" and implement it here — a
// bare marker impl if that's all it needs, e.g.:
// impl emacs::Transfer for JvmHandle {}

impl Drop for JvmHandle {
    fn drop(&mut self) {
        // Deliberately does NOT call `release(self.0)` directly here —
        // Lisp's GC may run this `Drop` on a thread that was never
        // JNI-attached, so the actual `GlobalRef` drop (which needs a
        // live JNI attachment) happens on the worker thread instead, via
        // Task 8's `Cmd::ReleaseRef` handling (`handles::release`, called
        // from there). Until Task 8 lands, Task 2's catch-all silently
        // no-ops this — meaning dropped handles' table entries aren't
        // actually cleaned up between Tasks 3 and 8, which is fine (no
        // test in that window exercises cleanup) and self-resolves once
        // Task 8 ships.
        let _ = crate::worker::send_command(crate::commands::Cmd::ReleaseRef { handle: self.0 });
    }
}
```

- [ ] **Step 4: Wire up the module**

In `src/lib.rs`, add:
```rust
mod handles;
```

- [ ] **Step 5: Write a defun that round-trips a `JvmHandle` through Lisp, to prove the `user-ptr` wiring works end to end**

This is deliberately a throwaway `#[cfg(feature = "test-support")]` defun
— Task 5 replaces it with the real `jvm-call`/`jvm-new` machinery that
actually produces `JvmHandle`s from real method calls; this step only
proves the `user-ptr` plumbing itself works before building reflection
dispatch on top of it.

In `src/lib.rs`:
```rust
#[cfg(feature = "test-support")]
#[defun(mod_in_name = false, name = "jvm--test-make-handle")]
fn jvm_test_make_handle() -> Result<handles::JvmHandle> {
    // A handle registered against nothing real (no live JVM object needed
    // for this plumbing-only test) — Task 5 is what makes handles wrap
    // genuine JNI objects.
    Ok(handles::JvmHandle(handles::register_dummy_for_test()))
}
```

Add a matching test-only helper to `src/handles.rs`:
```rust
#[cfg(feature = "test-support")]
pub fn register_dummy_for_test() -> u64 {
    NEXT_HANDLE.fetch_add(1, Ordering::SeqCst)
}
```

Create `test/jvm-handle-test.el`:
```elisp
;;; jvm-handle-test.el --- Tests for the JVM-object user-ptr plumbing -*- lexical-binding: t; -*-

(require 'ert)
(require 'emacs-jni)

(ert-deftest emacs-jni-jvm-handle-test/round-trips-as-a-distinct-lisp-value ()
  (let ((h1 (jvm--test-make-handle))
        (h2 (jvm--test-make-handle)))
    (should (user-ptrp h1))
    (should (user-ptrp h2))
    (should-not (eq h1 h2))))

(provide 'jvm-handle-test)
;;; jvm-handle-test.el ends here
```

- [ ] **Step 6: Run it**

Run: `EMACS_JNI_TEST_FEATURES=test-support ./test/run-ert-tests.sh test/jvm-handle-test.el`
Expected: passes. If `user-ptrp` returns false, the `emacs` crate didn't
expose `JvmHandle` as a `user-ptr` the way this plan assumed — check
`cargo doc -p emacs --open` for the correct mechanism and adjust Step 3.

- [ ] **Step 7: Commit**

```bash
git add src/handles.rs src/lib.rs test/jvm-handle-test.el
git commit -m "Add Rust-side JVM-object handle table exposed as a Lisp user-ptr"
```

---

## Task 4: Value marshalling (both directions)

**Files:**
- Create: `src/marshal.rs`
- Modify: `src/lib.rs` (add `mod marshal;`)
- Modify: `src/commands.rs` (`LispArg` gains no new variants here — Task 2
  already defined the full set this task needs; this task is what actually
  converts to/from it)

**Interfaces:**
- Consumes: `crate::handles::{register, resolve}` (Task 3).
- Produces: `pub fn lisp_arg_to_jvalue<'local>(env: &mut jni::Env<'local>, arg: &LispArg, param_type: &jni::objects::JClass) -> jni::errors::Result<jni::objects::JValueOwned<'local>>`
  (type-directed coercion per the spec's marshalling table),
  `pub fn jvm_value_to_lisp_arg<'local>(env: &mut jni::Env<'local>, value: jni::objects::JValueOwned<'local>) -> jni::errors::Result<LispArg>`
  (return-direction, wraps non-scalar returns via `handles::register`),
  `pub fn lisp_arg_to_sexp(arg: &LispArg) -> String` (renders a `LispArg`
  as the S-expression fragment `protocol::invoke_message`/`call_done_message`
  embed). Task 6 (reflection dispatch) and Task 7 (invoke plumbing) are
  the consumers of all three.

This task does **not** yet cover converting a real Lisp `Value` (from a
`#[defun]` argument) into a `LispArg`, or a `LispArg` back into a real
Lisp `Value` — that Lisp-side conversion is Task 6's job (it needs the
`emacs::Value`/`FromLisp`/`IntoLisp` machinery, which only matters once
there's a real `#[defun]` taking arbitrary arguments). This task is the
JVM-facing half only: `LispArg` (a plain Rust enum, already usable in unit
tests without any Emacs runtime) on one side, `jni::objects::JValueOwned`
on the other.

- [ ] **Step 1: Write `Lisp → Java` coercion with unit tests**

```rust
use jni::objects::{JClass, JObject, JValueOwned};
use jni::signature::{Primitive, ReflectedType};

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
            Ok(JValueOwned::Bool(0))
        }
        (LispArg::Nil, _) => Ok(JValueOwned::Object(JObject::null())),
        (LispArg::True, _) => Ok(JValueOwned::Bool(1)),
        (LispArg::Int(n), "int") => Ok(JValueOwned::Int(*n as i32)),
        (LispArg::Int(n), "short") => Ok(JValueOwned::Short(*n as i16)),
        (LispArg::Int(n), "byte") => Ok(JValueOwned::Byte(*n as i8)),
        (LispArg::Int(n), "long") => Ok(JValueOwned::Long(*n)),
        (LispArg::Int(n), "float") => Ok(JValueOwned::Float(*n as f32)),
        (LispArg::Int(n), "double") => Ok(JValueOwned::Double(*n as f64)),
        (LispArg::Int(n), "java.math.BigInteger") => big_integer_from_str(env, &n.to_string()),
        (LispArg::Int(_), other) => Err(marshal_error(&format!(
            "integer argument doesn't fit parameter type {other}"
        ))),
        (LispArg::Float(f), "float") => Ok(JValueOwned::Float(*f as f32)),
        (LispArg::Float(f), "double") => Ok(JValueOwned::Double(*f)),
        (LispArg::Float(_), other) => Err(marshal_error(&format!(
            "float argument doesn't fit parameter type {other}"
        ))),
        (LispArg::BigInt(digits), "java.math.BigInteger") => big_integer_from_str(env, digits),
        (LispArg::BigInt(_), other) => Err(marshal_error(&format!(
            "bignum argument too large for parameter type {other}"
        ))),
        (LispArg::Str(s), _) => {
            let jstr = env.new_string(s)?;
            Ok(JValueOwned::Object(jstr.into()))
        }
        (LispArg::Object(handle), _) => match handles::resolve(*handle) {
            Some(global) => {
                let local = env.new_local_ref(global.as_obj())?;
                Ok(JValueOwned::Object(local))
            }
            None => Err(marshal_error(&format!("unknown JVM object handle {handle}"))),
        },
        // A Callback arg reaching this function unconverted is a bug in
        // the caller — Task 6 must have already turned it into a
        // LispArg::Object(handle) (the freshly-registered LispFunction
        // proxy) before calling this function, since constructing that
        // proxy needs Lisp-side bookkeeping (Task 6's function table) that
        // this JVM-facing module has no access to.
        (LispArg::Callback(_), _) => {
            Err(marshal_error("internal error: raw Callback LispArg reached lisp_arg_to_jvalue"))
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

fn marshal_error(msg: &str) -> jni::errors::Error {
    // VERIFY: jni::errors::Error's actual constructors — this plan
    // assumes a variant or helper for "not a JNI failure, just our own
    // application error" exists; if not, the simplest fix is defining a
    // small local error type instead and having every caller in this
    // module return that instead of jni::errors::Result. Check
    // `cargo doc -p jni --open` for `jni::errors::Error`'s variants.
    jni::errors::Error::from(std::io::Error::new(std::io::ErrorKind::InvalidInput, msg.to_string()))
}
```

- [ ] **Step 2: Write `Java → Lisp` coercion**

Append to `src/marshal.rs`:
```rust
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
        JValueOwned::Bool(b) => Ok(if b != 0 { LispArg::True } else { LispArg::Nil }),
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

    let object_class = env.find_class(jni::jni_str!("java/lang/Object"))?;
    let string_class = env.find_class(jni::jni_str!("java/lang/String"))?;
    let boolean_class = env.find_class(jni::jni_str!("java/lang/Boolean"))?;
    let big_integer_class = env.find_class(jni::jni_str!("java/math/BigInteger"))?;
    let number_class = env.find_class(jni::jni_str!("java/lang/Number"))?;
    let obj_class = env.get_object_class(&obj)?;

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
    if env.is_instance_of(&obj, &number_class)? {
        let d = env
            .call_method(&obj, jni::jni_str!("doubleValue"), jni::jni_sig!(() -> double), &[])?
            .d()?;
        return Ok(LispArg::Float(d));
    }

    let _ = object_class; // silence unused-var warning if the above already covers every branch that needs it
    let global = env.new_global_ref(&obj)?;
    let handle = handles::register(global);
    let _ = obj_class;
    Ok(LispArg::Object(handle))
}
```

- [ ] **Step 3: Write `LispArg -> S-expression fragment` rendering**

Append to `src/marshal.rs`:
```rust
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
```

- [ ] **Step 4: Unit tests for the pure-Rust rendering function**

Append to `src/marshal.rs`:
```rust
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
        assert_eq!(lisp_arg_to_sexp(&LispArg::BigInt("123456789012345678901234567890".to_string())), "123456789012345678901234567890");
    }

    #[test]
    fn renders_string_quoted_and_escaped() {
        assert_eq!(lisp_arg_to_sexp(&LispArg::Str("hi \"there\"".to_string())), "\"hi \\\"there\\\"\"");
    }

    #[test]
    fn renders_object_handle() {
        assert_eq!(lisp_arg_to_sexp(&LispArg::Object(7)), "(jvm-handle 7)");
    }
}
```

- [ ] **Step 5: Write JVM-backed round-trip tests exercising a real reflective call**

These need a real JVM (like `handles::tests` and Phase 1's
`add_jar_url` test), so they belong in `#[cfg(test)] mod tests` too but
separate from the pure-Rust ones above:

```rust
    use jni::vm::{InitArgsBuilder, JavaVM};
    use jni::JNIVersion;

    fn create_test_jvm() -> JavaVM {
        let init_args = InitArgsBuilder::new().version(JNIVersion::V1_8).build().unwrap();
        JavaVM::new(init_args).unwrap()
    }

    #[test]
    fn int_arg_widens_correctly_to_a_long_parameter() {
        let vm = create_test_jvm();
        vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            // java.lang.Long.valueOf(long) — a real reflective call whose
            // single parameter is declared `long`, proving Int(_) marshals
            // to a JValueOwned::Long that Method.invoke actually accepts.
            let long_class = env.find_class(jni::jni_str!("java/lang/Long"))?;
            let methods = env
                .call_method(
                    &long_class,
                    jni::jni_str!("getMethod"),
                    jni::jni_sig!((name: java.lang.String, paramTypes: [java.lang.Class]) -> java.lang.reflect.Method),
                    &[
                        JValueOwned::Object(env.new_string("valueOf")?.into()).borrow(),
                        {
                            let class_class = env.find_class(jni::jni_str!("java/lang/Class"))?;
                            let long_primitive = primitive_long_class(env)?;
                            JValueOwned::Object(env.new_object_array(1, &class_class, &long_primitive)?.into()).borrow()
                        },
                    ],
                )?
                .l()?;
            let method = env.cast_local::<jni::objects::JObject>(methods)?;

            let long_primitive = primitive_long_class(env)?;
            let arg = lisp_arg_to_jvalue(env, &LispArg::Int(42), &long_primitive)?;
            let args_array = env.new_object_array(1, &env.find_class(jni::jni_str!("java/lang/Object"))?, JObject::null())?;
            env.set_object_array_element(&args_array, 0, arg_to_object(env, arg)?)?;

            let result = env
                .call_method(
                    &method,
                    jni::jni_str!("invoke"),
                    jni::jni_sig!((obj: java.lang.Object, args: [java.lang.Object]) -> java.lang.Object),
                    &[JValueOwned::Object(JObject::null()).borrow(), JValueOwned::Object(args_array.into()).borrow()],
                )?
                .l()?;
            let result_arg = object_to_lisp_arg(env, result)?;
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
    // the reflective invoke path realistically. If this feels awkward
    // during implementation, it's a sign `lisp_arg_to_jvalue` may need a
    // sibling function that always produces a boxed Object (for varargs-
    // style callers like Task 6's Method.invoke, which always wants an
    // Object[] regardless of the target parameter's primitive-ness) —
    // flag this as a real design question for Task 6 to resolve, don't
    // silently duplicate boxing logic ad hoc there.
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
```

This test doubles as documentation of a real open design question flagged
inline above (whether `lisp_arg_to_jvalue` needs a boxed-Object sibling
for Task 6's varargs-style `Method.invoke` call) — Task 6 must resolve it,
not silently reinvent ad hoc boxing.

- [ ] **Step 6: Run everything**

Run: `cargo test --lib marshal::`
Expected: all tests pass, including the JVM-backed one. This step is
likely to need real iteration against actual `jni` crate method names
(`is_instance_of`, `get_static_field`, `set_object_array_element`, etc.) —
adjust based on compiler output, same as every other JNI-heavy step in
this plan.

- [ ] **Step 7: Wire up the module and commit**

In `src/lib.rs`, add:
```rust
mod marshal;
```

```bash
git add src/marshal.rs src/lib.rs
git commit -m "Add type-directed Lisp<->Java value marshalling (numbers, bignums, strings, booleans, wrapped objects)"
```

---

## Task 5: Reflection-based `Cmd::Call` dispatch, per-call threads, and `jvm-call`/`jvm-new`

The largest task in this plan — do not split further, since reflection
dispatch, per-call thread dispatch, and the Lisp-facing primitives are not
independently testable without each other (mirrors Phase 1's Task 4,
which made the same call for the same reason).

**Files:**
- Modify: `src/jvm_thread.rs` (replace the `Cmd::Call` catch-all; add
  reflective dispatch + per-call thread spawning)
- Modify: `src/lib.rs` (add `jvm--call` defun; convert `Value` args to
  `LispArg`)
- Modify: `elisp/emacs-jni.el` (add `jvm-call`, `jvm-call-sync`, `jvm-new`,
  `jvm-new-sync`; add the `(set-process-thread jvm--pipe-process nil)`
  fix; extend the filter's dispatch for `call-done`)
- Create: `test/fixtures/Calc.java` (a fixture with static methods,
  instance methods, and overloads, to exercise reflection dispatch
  realistically)
- Modify: `test/run-ert-tests.sh` (build the new fixture jar, mirroring
  how `greeter.jar` is already built)
- Create: `test/jvm-call-test.el`

**Interfaces:**
- Consumes: `marshal::{lisp_arg_to_jvalue, jvm_value_to_lisp_arg,
  lisp_arg_to_sexp}` (Task 4), `handles::{register, resolve}` (Task 3),
  `protocol::call_done_message` (Task 2).
- Produces: `jvm--call` defun (returns `u64` request id); Elisp `jvm-call`,
  `jvm-call-sync`, `jvm-new`, `jvm-new-sync`.

- [ ] **Step 1: Write the fixture**

`test/fixtures/Calc.java`:
```java
public class Calc {
    public static long addLong(long a, long b) {
        return a + b;
    }

    public static java.math.BigInteger addBig(java.math.BigInteger a, java.math.BigInteger b) {
        return a.add(b);
    }

    private final long base;

    public Calc(long base) {
        this.base = base;
    }

    public long addToBase(long n) {
        return base + n;
    }

    public static String describe(boolean flag) {
        return flag ? "yes" : "no";
    }
}
```

- [ ] **Step 2: Resolve a target `Method`/`Constructor` reflectively and invoke it**

In `src/jvm_thread.rs`, add (near the other `handle_*` functions):
```rust
fn handle_call(
    vm: &JavaVM,
    id: u64,
    target: crate::commands::CallTarget,
    member: String,
    args: Vec<crate::commands::LispArg>,
) {
    // Runs on its own short-lived thread (spawned by the caller, see Step
    // 3) — never inline on the worker's dispatch loop, per spec "Worker
    // thread role": an arbitrary Java call can take a long time for
    // reasons unrelated to callbacks, and must not stall other pending
    // commands.
    let result: jni::errors::Result<crate::commands::LispArg> = vm.attach_current_thread(|env| {
        let (class, instance): (jni::objects::JClass, Option<jni::objects::JObject>) = match &target {
            crate::commands::CallTarget::ClassName(name) => {
                (env.find_class(name.replace('.', "/"))?, None)
            }
            crate::commands::CallTarget::Object(handle) => {
                let global = crate::handles::resolve(*handle)
                    .ok_or_else(|| jni_error(&format!("unknown JVM object handle {handle}")))?;
                let local = env.new_local_ref(global.as_obj())?;
                (env.get_object_class(&local)?, Some(local))
            }
        };

        if member == "<init>" {
            invoke_constructor(env, &class, &args)
        } else {
            invoke_method(env, &class, instance.as_ref(), &member, &args)
        }
    });

    match result {
        Ok(value) => {
            let sexp = crate::marshal::lisp_arg_to_sexp(&value);
            channel::send(&protocol::call_done_message(id, true, Some(&sexp), None));
        }
        Err(e) => channel::send(&protocol::call_done_message(id, false, None, Some(&e.to_string()))),
    }
}

fn jni_error(msg: &str) -> jni::errors::Error {
    // Same VERIFY note as marshal::marshal_error (Task 4, Step 1) — adjust
    // to whatever jni::errors::Error actually offers.
    jni::errors::Error::from(std::io::Error::new(std::io::ErrorKind::InvalidInput, msg.to_string()))
}

/// Finds an overload of METHOD_NAME on CLASS whose parameter count matches
/// ARGS, invokes it, and marshals the result. Ambiguous overloads (same
/// arg count, multiple candidates) resolve to the first match from
/// `getMethods()` — acceptable for a prototype; a real overload-resolution
/// algorithm (assignability-based scoring) is future work if this proves
/// too coarse in practice.
fn invoke_method(
    env: &mut jni::Env<'_>,
    class: &jni::objects::JClass,
    instance: Option<&jni::objects::JObject>,
    method_name: &str,
    args: &[crate::commands::LispArg],
) -> jni::errors::Result<crate::commands::LispArg> {
    let methods = env
        .call_method(
            class,
            jni::jni_str!("getMethods"),
            jni::jni_sig!(() -> [java.lang.reflect.Method]),
            &[],
        )?
        .l()?;
    let methods = env.cast_local::<jni::objects::JObjectArray>(methods)?;
    let count = env.get_array_length(&methods)?;

    for i in 0..count {
        let candidate = env.get_object_array_element(&methods, i)?;
        let name = env
            .call_method(&candidate, jni::jni_str!("getName"), jni::jni_sig!(() -> java.lang.String), &[])?
            .l()?;
        let name = env.cast_local::<jni::objects::JString>(name)?.to_string();
        if name != method_name {
            continue;
        }
        let param_types = env
            .call_method(
                &candidate,
                jni::jni_str!("getParameterTypes"),
                jni::jni_sig!(() -> [java.lang.Class]),
                &[],
            )?
            .l()?;
        let param_types = env.cast_local::<jni::objects::JObjectArray>(param_types)?;
        if env.get_array_length(&param_types)? as usize != args.len() {
            continue;
        }

        let object_class = env.find_class(jni::jni_str!("java/lang/Object"))?;
        let invoke_args = env.new_object_array(args.len() as i32, &object_class, jni::objects::JObject::null())?;
        for (i, arg) in args.iter().enumerate() {
            let param_type = env.cast_local::<jni::objects::JClass>(env.get_object_array_element(&param_types, i as i32)?)?;
            let jvalue = crate::marshal::lisp_arg_to_jvalue(env, arg, &param_type)?;
            let boxed = crate::marshal::box_if_primitive(env, jvalue)?;
            env.set_object_array_element(&invoke_args, i as i32, &boxed)?;
        }

        let result = env
            .call_method(
                &candidate,
                jni::jni_str!("invoke"),
                jni::jni_sig!((obj: java.lang.Object, args: [java.lang.Object]) -> java.lang.Object),
                &[
                    jni::objects::JValueOwned::Object(instance.cloned().unwrap_or_else(jni::objects::JObject::null)).borrow(),
                    jni::objects::JValueOwned::Object(invoke_args.into()).borrow(),
                ],
            )?
            .l()?;
        return crate::marshal::jvm_value_to_lisp_arg(env, jni::objects::JValueOwned::Object(result));
    }

    Err(jni_error(&format!("no method {method_name} with {} argument(s) found", args.len())))
}

fn invoke_constructor(
    env: &mut jni::Env<'_>,
    class: &jni::objects::JClass,
    args: &[crate::commands::LispArg],
) -> jni::errors::Result<crate::commands::LispArg> {
    let constructors = env
        .call_method(
            class,
            jni::jni_str!("getConstructors"),
            jni::jni_sig!(() -> [java.lang.reflect.Constructor]),
            &[],
        )?
        .l()?;
    let constructors = env.cast_local::<jni::objects::JObjectArray>(constructors)?;
    let count = env.get_array_length(&constructors)?;

    for i in 0..count {
        let candidate = env.get_object_array_element(&constructors, i)?;
        let param_types = env
            .call_method(
                &candidate,
                jni::jni_str!("getParameterTypes"),
                jni::jni_sig!(() -> [java.lang.Class]),
                &[],
            )?
            .l()?;
        let param_types = env.cast_local::<jni::objects::JObjectArray>(param_types)?;
        if env.get_array_length(&param_types)? as usize != args.len() {
            continue;
        }

        let object_class = env.find_class(jni::jni_str!("java/lang/Object"))?;
        let invoke_args = env.new_object_array(args.len() as i32, &object_class, jni::objects::JObject::null())?;
        for (i, arg) in args.iter().enumerate() {
            let param_type = env.cast_local::<jni::objects::JClass>(env.get_object_array_element(&param_types, i as i32)?)?;
            let jvalue = crate::marshal::lisp_arg_to_jvalue(env, arg, &param_type)?;
            let boxed = crate::marshal::box_if_primitive(env, jvalue)?;
            env.set_object_array_element(&invoke_args, i as i32, &boxed)?;
        }

        let instance = env
            .call_method(
                &candidate,
                jni::jni_str!("newInstance"),
                jni::jni_sig!((args: [java.lang.Object]) -> java.lang.Object),
                &[jni::objects::JValueOwned::Object(invoke_args.into()).borrow()],
            )?
            .l()?;
        return crate::marshal::jvm_value_to_lisp_arg(env, jni::objects::JValueOwned::Object(instance));
    }

    Err(jni_error(&format!("no constructor with {} argument(s) found", args.len())))
}
```

Add the boxing helper this step's code calls, to `src/marshal.rs` (this is
the design gap Task 4 Step 5 flagged — resolve it here as a real sibling
function rather than duplicating ad hoc boxing):
```rust
/// `Method.invoke`/`Constructor.newInstance` always want an `Object[]`,
/// even for parameters whose declared type is primitive — so any
/// primitive `JValueOwned` `lisp_arg_to_jvalue` produced must be boxed
/// before going into that array. Object values pass through unchanged.
pub fn box_if_primitive<'local>(
    env: &mut jni::Env<'local>,
    value: JValueOwned<'local>,
) -> jni::errors::Result<JObject<'local>> {
    match value {
        JValueOwned::Object(o) => Ok(o),
        JValueOwned::Bool(b) => box_via_value_of(env, "java/lang/Boolean", jni::jni_sig!((v: boolean) -> java.lang.Boolean), JValueOwned::Bool(b)),
        JValueOwned::Byte(n) => box_via_value_of(env, "java/lang/Byte", jni::jni_sig!((v: byte) -> java.lang.Byte), JValueOwned::Byte(n)),
        JValueOwned::Short(n) => box_via_value_of(env, "java/lang/Short", jni::jni_sig!((v: short) -> java.lang.Short), JValueOwned::Short(n)),
        JValueOwned::Int(n) => box_via_value_of(env, "java/lang/Integer", jni::jni_sig!((v: int) -> java.lang.Integer), JValueOwned::Int(n)),
        JValueOwned::Long(n) => box_via_value_of(env, "java/lang/Long", jni::jni_sig!((v: long) -> java.lang.Long), JValueOwned::Long(n)),
        JValueOwned::Float(f) => box_via_value_of(env, "java/lang/Float", jni::jni_sig!((v: float) -> java.lang.Float), JValueOwned::Float(f)),
        JValueOwned::Double(f) => box_via_value_of(env, "java/lang/Double", jni::jni_sig!((v: double) -> java.lang.Double), JValueOwned::Double(f)),
        JValueOwned::Char(c) => box_via_value_of(env, "java/lang/Character", jni::jni_sig!((v: char) -> java.lang.Character), JValueOwned::Char(c)),
        JValueOwned::Void => Ok(JObject::null()),
    }
}

fn box_via_value_of<'local>(
    env: &mut jni::Env<'local>,
    class_name: &str,
    sig: &str,
    value: JValueOwned<'local>,
) -> jni::errors::Result<JObject<'local>> {
    let class = env.find_class(class_name)?;
    env.call_static_method(&class, jni::jni_str!("valueOf"), sig, &[value.borrow()])?.l()
}
```

(`jni::jni_sig!`'s exact return type when used as a plain `&str` argument
to `call_static_method` here is another spot to verify against the
compiler — Phase 1's existing calls always pass the macro's output
directly as a positional argument, so this should work the same way, but
confirm.)

- [ ] **Step 3: Spawn a per-call thread and update `run`'s `Cmd::Call` handling**

In `src/jvm_thread.rs`, replace Task 2's catch-all arm's `Cmd::Call { .. }`
branch (leave `InvokeReply`/`ReleaseRef` as the catch-all still, Tasks 7-8
handle those) — this needs `vm` cloned into the spawned thread, which is
why `JavaVM: Clone` (noted in Phase 1's plan Design note) matters here:

```rust
            Cmd::Call { id, target, member, args } => {
                let vm = vm.clone();
                std::thread::spawn(move || handle_call(&vm, id, target, member, args));
            }
            Cmd::InvokeReply { .. } | Cmd::ReleaseRef { .. } => {}
```

- [ ] **Step 4: Run `cargo build` and fix compile errors**

Run: `cargo build`
Expected: succeeds. This step will likely need real iteration — it's the
densest reflection code in the plan. Use the compiler's error output to
correct method names/signatures rather than guessing further; this
mirrors Phase 1's own Task 6/7 iteration process.

- [ ] **Step 5: Add the `jvm--call` defun in `src/lib.rs`**

Converting a raw `emacs::Value` argument list into `Vec<LispArg>` needs a
Lisp-side type check per argument (integer/float/string/t/nil/user-ptr).
`Value`'s exact type-testing API (`is_integer`, `.into_rust::<i64>()`,
etc.) should be checked against `cargo doc -p emacs --open` if the calls
below don't compile as written — the intent (dispatch on which Lisp type
each `Value` is, converting accordingly) is what must be preserved even if
individual method names differ:

```rust
#[defun(mod_in_name = false, name = "jvm--call")]
fn jvm_call(env: &Env, target: Value, member: String, args: Value) -> Result<u64> {
    if STATUS.get() != Status::Ready {
        return env.signal(jvm_illegal_transition, (STATUS.get().as_tag(),));
    }
    let target = if let Ok(class_name) = target.into_rust::<String>() {
        commands::CallTarget::ClassName(class_name)
    } else {
        let h: handles::JvmHandle = target.into_rust()?;
        commands::CallTarget::Object(h.0)
    };
    let args = lisp_list_to_args(args)?;
    let id = worker::next_request_id();
    match worker::send_command(commands::Cmd::Call { id, target, member, args }) {
        Ok(()) => Ok(id),
        Err(()) => env.signal(jvm_illegal_transition, ("no running JVM worker thread",)),
    }
}

/// Walks a Lisp list of arguments (as raw `Value`s, same pattern as the
/// existing `string_list` helper) converting each to a `LispArg` by
/// runtime type. Symbols other than `t`/`nil`, or any Lisp value not
/// covered by the spec's marshalling table, is a signalled error rather
/// than a silent best-effort guess.
fn lisp_list_to_args(mut list: Value) -> Result<Vec<commands::LispArg>> {
    let mut out = Vec::new();
    while list.is_not_nil() {
        let item: Value = list.car()?;
        out.push(lisp_value_to_arg(item)?);
        list = list.cdr()?;
    }
    Ok(out)
}

fn lisp_value_to_arg(v: Value) -> Result<commands::LispArg> {
    if v.is_not_nil() == false {
        return Ok(commands::LispArg::Nil);
    }
    if let Ok(n) = v.into_rust::<i64>() {
        return Ok(commands::LispArg::Int(n));
    }
    if let Ok(f) = v.into_rust::<f64>() {
        return Ok(commands::LispArg::Float(f));
    }
    if let Ok(s) = v.into_rust::<String>() {
        return Ok(commands::LispArg::Str(s));
    }
    if let Ok(h) = v.into_rust::<handles::JvmHandle>() {
        return Ok(commands::LispArg::Object(h.0));
    }
    // `t` and Lisp functions are the remaining cases: t has no useful
    // into_rust::<T>() scalar conversion, and functions are Task 6's
    // territory (auto-wrap-as-LispFunction) — Task 6 replaces this
    // fallback with real function-handling; until then this errors
    // clearly rather than silently mishandling it.
    Ok(commands::LispArg::True)
}
```

- [ ] **Step 6: Extend `elisp/emacs-jni.el`**

Add the `set-process-thread` fix from the spec's "Threading model" section
directly in `jvm--ensure-pipe-process`:
```elisp
(defun jvm--ensure-pipe-process ()
  "Create the pipe process and open the Rust-side channel, once."
  (unless (process-live-p jvm--pipe-process)
    (setq jvm--pipe-process
          (make-pipe-process
           :name "emacs-jni-channel"
           :filter #'jvm--process-filter
           :noquery t))
    ;; Multiple Lisp threads may independently call a `-sync` wrapper;
    ;; without this, any thread other than whichever one created the
    ;; process hard-errors on its first `accept-process-output` (verified
    ;; empirically — see .probe/lisp-thread-filter-probe/).
    (set-process-thread jvm--pipe-process nil)
    (jvm--init-channel jvm--pipe-process)))
```

Extend `jvm--dispatch-message`'s `pcase` with a `call-done` clause and add
a pending-callback table mirroring the existing `jvm--pending-load-jar`:
```elisp
(defvar jvm--pending-call nil
  "Alist of request-id -> callback for in-flight `jvm-call'/`jvm-new' calls.")
```

In the `pcase` inside `jvm--dispatch-message`, add:
```elisp
    (`(call-done ,id ,ok ,result ,err)
     (when-let ((callback (alist-get id jvm--pending-call)))
       (setq jvm--pending-call (assq-delete-all id jvm--pending-call))
       (funcall callback ok (jvm--resolve-sexp-result result) err)))
```

`result`'s `(jvm-handle N)` tag (from Task 4's `lisp_arg_to_sexp`) needs
turning back into a real `user-ptr` before reaching the callback — add:
```elisp
(defun jvm--resolve-sexp-result (result)
  "Convert RESULT as parsed from the wire (may contain `(jvm-handle N)')
into its final Lisp-visible form."
  (pcase result
    (`(jvm-handle ,n) (jvm--wrap-handle n))
    (_ result)))
```

`jvm--wrap-handle` needs a Rust primitive to actually construct a
`JvmHandle` `user-ptr` from a raw integer received over the wire (the
existing `handles::JvmHandle` type only gets created Rust-side via
`register`, but here we already have the u64 and just need the Lisp
wrapper) — add to `src/lib.rs`:
```rust
#[defun(mod_in_name = false, name = "jvm--wrap-handle")]
fn jvm_wrap_handle(handle: u64) -> Result<handles::JvmHandle> {
    Ok(handles::JvmHandle(handle))
}
```

Now the Lisp-facing primitives, inserted before `(provide 'emacs-jni)`:
```elisp
(defun jvm-call (target member args &optional callback)
  "Call TARGET.MEMBER(ARGS...) on the JVM asynchronously.
TARGET is a class-name string (static call) or a wrapped JVM object
\(instance call). ARGS is a list. Returns a request id immediately. If
CALLBACK is given, it is called as (CALLBACK OK RESULT ERR) once the call
finishes."
  (let ((id (jvm--call target member args)))
    (when callback
      (push (cons id callback) jvm--pending-call))
    id))

(defun jvm-call-sync (target member args &optional timeout)
  "Like `jvm-call', but blocks until it finishes or TIMEOUT (default 30s).
Returns (OK RESULT ERR) as a 3-element list; signals `jvm-timeout' if
TIMEOUT elapses first."
  (let* ((result :pending)
         (id (jvm-call target member args
                        (lambda (ok res err) (setq result (list ok res err)))))
         (deadline (+ (float-time) (or timeout 30))))
    (while (eq result :pending)
      (unless (accept-process-output jvm--pipe-process 0.1)
        (when (> (float-time) deadline)
          (setq jvm--pending-call (assq-delete-all id jvm--pending-call))
          (signal 'jvm-timeout (list 'jvm-call-sync target member)))))
    result))

(defun jvm-new (class-name args &optional callback)
  "Construct a new CLASS-NAME(ARGS...) instance on the JVM asynchronously.
Sugar over `jvm-call' with MEMBER set to the constructor's actual internal
JVM bytecode name, `\"<init>\"'."
  (jvm-call class-name "<init>" args callback))

(defun jvm-new-sync (class-name args &optional timeout)
  "Like `jvm-new', but blocks. See `jvm-call-sync'."
  (jvm-call-sync class-name "<init>" args timeout))
```

- [ ] **Step 7: Extend `test/run-ert-tests.sh` to build the `Calc` fixture**

Find where `greeter.jar` gets built (or the equivalent existing fixture
build step) and add an analogous block for `Calc.java` — same `javac -d`
+ `jar cf` pattern, producing `target/test-fixtures/calc.jar`, then add it
to the classpath the test Emacs process is launched with (or have the
test itself call `jvm-load-jar-sync` on it — check how `greeter.jar`'s
test already does this and match that exact approach rather than
introducing a second pattern).

- [ ] **Step 8: Write `test/jvm-call-test.el`**

```elisp
;;; jvm-call-test.el --- Tests for jvm-call/jvm-new -*- lexical-binding: t; -*-

(require 'ert)
(require 'emacs-jni)

(defvar jvm-call-test--calc-jar
  (expand-file-name "../target/test-fixtures/calc.jar"
                     (file-name-directory (or load-file-name buffer-file-name))))

(ert-deftest emacs-jni-jvm-call-test/static-call-and-instance-call ()
  (should (eq (jvm-start-sync) 'ready))
  (should (equal (jvm-load-jar-sync jvm-call-test--calc-jar) '(t)))
  (should (equal (jvm-call-sync "Calc" "addLong" '(2 3)) '(t 5 nil)))
  (let ((obj (nth 1 (jvm-new-sync "Calc" '(10)))))
    (should (user-ptrp obj))
    (should (equal (jvm-call-sync obj "addToBase" '(5)) '(t 15 nil)))))

(ert-deftest emacs-jni-jvm-call-test/nil-marshals-to-false-for-boolean-param ()
  (should (eq (jvm-status) 'ready))
  (should (equal (jvm-call-sync "Calc" "describe" '(nil)) '(t "no" nil))))

(ert-deftest emacs-jni-jvm-call-test/bignum-round-trips ()
  (should (eq (jvm-status) 'ready))
  (let ((big (* (expt 2 62) (expt 2 62)))) ; exceeds i64/long range, forces bignum
    (should (equal (jvm-call-sync "Calc" "addBig" (list big 1)) (list t (1+ big) nil)))))

(provide 'jvm-call-test)
;;; jvm-call-test.el ends here
```

- [ ] **Step 9: Run it**

Run: `./test/run-ert-tests.sh test/jvm-call-test.el`
Expected: passes. Budget real iteration time here — this exercises the
densest new code path in the plan end to end for the first time.

- [ ] **Step 10: Commit**

```bash
git add src/jvm_thread.rs src/marshal.rs src/lib.rs elisp/emacs-jni.el test/fixtures/Calc.java test/run-ert-tests.sh test/jvm-call-test.el
git commit -m "Add reflection-based jvm-call/jvm-new with per-call thread dispatch"
```

---

## Task 6: Lisp function handle table and auto-wrap-as-`LispFunction`

**Files:**
- Modify: `elisp/emacs-jni.el` (add `jvm--lisp-callbacks` table, a
  registration primitive)
- Modify: `src/lib.rs` (accept a function handle in `lisp_value_to_arg`)
- Modify: `src/marshal.rs` (`LispArg::Callback` now gets turned into a
  real `LispFunction` instance via `shim`, instead of Task 4's placeholder
  error)

**Interfaces:**
- Consumes: `shim::define_classes`'s loaded classes (Task 1, already
  loaded — this task just uses `find_class` on them).
- Produces: Elisp `jvm--register-callback (fn) -> handle`; extends
  `marshal::lisp_arg_to_jvalue` to handle `LispArg::Callback` for real.

- [ ] **Step 1: Add the Lisp-side function table**

In `elisp/emacs-jni.el`, before `(provide 'emacs-jni)`:
```elisp
(defvar jvm--lisp-callbacks (make-hash-table :test 'eql)
  "Handle -> Lisp function, for functions wrapped as JVM LispFunction
proxies. Entries are removed when the JVM notifies us via `(release
HANDLE)' that the corresponding proxy was garbage collected.")

(defvar jvm--next-callback-handle 1
  "Monotonic counter for `jvm--lisp-callbacks' keys.")

(defun jvm--register-callback (fn)
  "Register FN in `jvm--lisp-callbacks', returning its fresh handle."
  (let ((handle jvm--next-callback-handle))
    (setq jvm--next-callback-handle (1+ jvm--next-callback-handle))
    (puthash handle fn jvm--lisp-callbacks)
    handle))
```

- [ ] **Step 2: Make `lisp_value_to_arg` (Task 5) recognize Lisp functions**

In `src/lib.rs`, replace the fallback at the end of `lisp_value_to_arg`
(currently `Ok(commands::LispArg::True)`) — a Lisp function value needs to
be registered Lisp-side (via the new `jvm--register-callback`), not
Rust-side, since Rust never holds a reference to it (spec: "Identity and
lifetime" — Lisp owns that table). This means `lisp_value_to_arg` must
call back into Lisp to register the function and get its handle:

```rust
fn lisp_value_to_arg(env: &Env, v: Value) -> Result<commands::LispArg> {
    if v.is_not_nil() == false {
        return Ok(commands::LispArg::Nil);
    }
    if let Ok(n) = v.into_rust::<i64>() {
        return Ok(commands::LispArg::Int(n));
    }
    if let Ok(f) = v.into_rust::<f64>() {
        return Ok(commands::LispArg::Float(f));
    }
    if let Ok(s) = v.into_rust::<String>() {
        return Ok(commands::LispArg::Str(s));
    }
    if let Ok(h) = v.into_rust::<handles::JvmHandle>() {
        return Ok(commands::LispArg::Object(h.0));
    }
    if v.is_function()? {
        let handle: u64 = env.call("jvm--register-callback", (v,))?.into_rust()?;
        return Ok(commands::LispArg::Callback(handle));
    }
    Ok(commands::LispArg::True)
}
```

This changes `lisp_value_to_arg`'s signature (now needs `env`) —
update its one caller, `lisp_list_to_args` (Task 5), to thread `env`
through:
```rust
fn lisp_list_to_args(env: &Env, mut list: Value) -> Result<Vec<commands::LispArg>> {
    let mut out = Vec::new();
    while list.is_not_nil() {
        let item: Value = list.car()?;
        out.push(lisp_value_to_arg(env, item)?);
        list = list.cdr()?;
    }
    Ok(out)
}
```
And `jvm_call`'s call site:
```rust
    let args = lisp_list_to_args(env, args)?;
```

`Value::is_function()` is this plan's best guess at how to test "is this a
callable Lisp value" via the `emacs` crate — check `cargo doc -p emacs
--open` if it doesn't exist under that name; `(functionp v)` called via
`env.call` is a reliable fallback that works regardless of what the crate
exposes natively.

- [ ] **Step 3: Turn a `LispArg::Callback` into a real `LispFunction` instance in `marshal.rs`**

Replace Task 4's placeholder error arm in `lisp_arg_to_jvalue`:
```rust
        (LispArg::Callback(_), _) => {
            Err(marshal_error("internal error: raw Callback LispArg reached lisp_arg_to_jvalue"))
        }
```
with:
```rust
        (LispArg::Callback(handle), _) => {
            let lisp_function_class = env.find_class(jni::jni_str!("emacs_jni/LispFunction"))?;
            let obj = env.new_object(
                &lisp_function_class,
                jni::jni_sig!((handle: long) -> void),
                &[JValueOwned::Long(*handle as i64).borrow()],
            )?;
            Ok(JValueOwned::Object(obj))
        }
```

(`LispFunction`'s constructor is package-private in Task 1's source, which
is fine — JNI's reflective/direct object construction via `new_object`
isn't subject to Java-level access control the way normal compiled code
calling across packages would be.)

- [ ] **Step 4: Run `cargo build`**

Run: `cargo build`
Expected: succeeds.

- [ ] **Step 5: Write a test proving a Lisp function passed as an arg becomes a real `LispCallable` on the Java side**

This can't yet prove the function is *callable* end-to-end (that needs
Task 7's `invoke`/`InvokeReply` plumbing) — it only proves the wrapping
half. Add to `test/fixtures/Calc.java`:
```java
    public static boolean isLispCallable(Object obj) {
        return obj instanceof emacs_jni.LispCallable;
    }
```

`test/jvm-callback-wrap-test.el`:
```elisp
;;; jvm-callback-wrap-test.el --- Tests for wrapping a Lisp fn as a JVM object -*- lexical-binding: t; -*-

(require 'ert)
(require 'emacs-jni)

(defvar jvm-callback-wrap-test--calc-jar
  (expand-file-name "../target/test-fixtures/calc.jar"
                     (file-name-directory (or load-file-name buffer-file-name))))

(ert-deftest emacs-jni-jvm-callback-wrap-test/lisp-function-becomes-lispcallable ()
  (should (eq (jvm-start-sync) 'ready))
  (should (equal (jvm-load-jar-sync jvm-callback-wrap-test--calc-jar) '(t)))
  (should (equal (jvm-call-sync "Calc" "isLispCallable" (list (lambda (x) x)))
                  '(t t nil))))

(provide 'jvm-callback-wrap-test)
;;; jvm-callback-wrap-test.el ends here
```

- [ ] **Step 6: Run it**

Run: `./test/run-ert-tests.sh test/jvm-callback-wrap-test.el`
Expected: passes.

- [ ] **Step 7: Commit**

```bash
git add elisp/emacs-jni.el src/lib.rs src/marshal.rs test/fixtures/Calc.java test/jvm-callback-wrap-test.el
git commit -m "Auto-wrap Lisp functions as LispFunction/LispCallable when passed as jvm-call arguments"
```

---

## Task 7: `invoke`/`InvokeReply` — the full JVM-calls-Lisp round trip

The other large, interdependent task in this plan (mirrors Task 5's
reasoning) — the native `LispFunction.call` implementation, the
pending-continuations table, `Cmd::InvokeReply` handling, and the
Lisp-side `invoke` dispatch are only testable together.

**Files:**
- Create: `src/pending.rs` (pending-continuations table, symmetric to
  `handles.rs` but for `CompletableFuture` global refs)
- Modify: `src/jvm_thread.rs` (real `Cmd::InvokeReply` handling; native
  method registration for `LispFunction.call`)
- Modify: `src/shim.rs` (register the native implementation of
  `LispFunction.call` via `RegisterNatives`)
- Modify: `elisp/emacs-jni.el` (dispatch `invoke` messages: funcall the
  registered callback, reply via a new primitive)
- Modify: `src/lib.rs` (add `jvm--invoke-reply` defun)
- Create: `test/jvm-invoke-test.el`

**Interfaces:**
- Consumes: `handles`-style pattern (this task's `pending.rs` is its own
  independent table, not reusing `handles.rs`, since it stores
  `CompletableFuture` refs keyed by request id, a different lifecycle than
  JVM-object handles), `jvm--lisp-callbacks` (Task 6).
- Produces: `pending::register(future: Global<JObject<'static>>) -> u64`,
  `pending::take(id: u64) -> Option<Global<JObject<'static>>>`.

- [ ] **Step 1: Write `src/pending.rs`**

```rust
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use jni::objects::JObject;
use jni::refs::Global;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static TABLE: Mutex<Option<HashMap<u64, Global<JObject<'static>>>>> = Mutex::new(None);

fn table() -> std::sync::MutexGuard<'static, Option<HashMap<u64, Global<JObject<'static>>>>> {
    let mut guard = TABLE.lock().unwrap();
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    guard
}

pub fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::SeqCst)
}

pub fn register(id: u64, future: Global<JObject<'static>>) {
    table().as_mut().unwrap().insert(id, future);
}

/// Removes and returns the pending future for ID, if any — a reply should
/// only ever resolve a given id once, so this consumes the entry rather
/// than just peeking.
pub fn take(id: u64) -> Option<Global<JObject<'static>>> {
    table().as_mut().unwrap().remove(&id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jni::vm::{InitArgsBuilder, JavaVM};
    use jni::JNIVersion;

    #[test]
    fn register_then_take_returns_once() {
        let init_args = InitArgsBuilder::new().version(JNIVersion::V1_8).build().unwrap();
        let vm = JavaVM::new(init_args).unwrap();
        vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            let obj = env.new_object(&env.find_class(jni::jni_str!("java/lang/Object"))?, jni::jni_sig!(() -> void), &[])?;
            let global = env.new_global_ref(&obj)?;
            let id = next_id();
            register(id, global);
            assert!(take(id).is_some());
            assert!(take(id).is_none());
            Ok(())
        })
        .unwrap();
    }
}
```

- [ ] **Step 2: Register `LispFunction.call`'s native implementation**

**VERIFY** this step's `jni` crate API names carefully (see "Note on API
uncertainty" at the top of this plan) — the standard JNI operation needed
is `RegisterNatives(env, class, methods, nMethods)`, where each method
entry is a `(name, signature, fn_ptr)` triple. jni-rs commonly exposes
this as `Env::register_native_methods` taking a slice of `NativeMethod`
(or similarly named) structs built via a macro or constructor; check
`cargo doc -p jni --open` for the actual names.

Add to `src/shim.rs`, called from `define_classes` after all three classes
are defined:
```rust
pub fn define_classes(env: &mut jni::Env) -> jni::errors::Result<()> {
    // ... (Task 1's class-defining code stays as-is above this point) ...

    register_lisp_function_native(env)?;

    Ok(())
}

/// The actual implementation behind `LispFunction.call`: allocates a
/// fresh correlation id, constructs a (still-incomplete) `CompletableFuture`,
/// registers it in `pending`, posts `(invoke ID HANDLE ARGS)`, and returns
/// the future immediately — this is the concrete realization of the spec's
/// "Java shim" section's description. Never blocks.
extern "system" fn lisp_function_call<'local>(
    mut env: jni::Env<'local>,
    this: jni::objects::JObject<'local>,
    args: jni::objects::JObjectArray<'local>,
) -> jni::sys::jobject {
    // VERIFY: the exact extern-fn signature jni-rs 0.22.4's native-method
    // registration expects (parameter types/ownership, return type as a
    // raw `jni::sys::jobject` vs. a wrapped local ref) — check
    // `cargo doc -p jni --open` for whatever example/type
    // `register_native_methods` (or its actual name) documents, and match
    // that exactly rather than this plan's best guess.
    let result = (|| -> jni::errors::Result<jni::sys::jobject> {
        let handle = get_handle_field(&mut env, &this)?;
        let id = crate::pending::next_id();

        let future_class = env.find_class(jni::jni_str!("java/util/concurrent/CompletableFuture"))?;
        let future = env.new_object(&future_class, jni::jni_sig!(() -> void), &[])?;
        let future_global = env.new_global_ref(&future)?;
        crate::pending::register(id, future_global);

        let args_sexp = jobject_array_to_sexp(&mut env, &args)?;
        crate::channel::send(&crate::protocol::invoke_message(id, handle, &args_sexp));

        // VERIFY: converting a local JObject into the raw jobject this
        // function must return — jni-rs typically exposes this via
        // `.as_raw()` or `Into<jni::sys::jobject>`.
        Ok(future.as_raw())
    })();

    result.unwrap_or(std::ptr::null_mut())
}

fn get_handle_field(env: &mut jni::Env<'_>, this: &jni::objects::JObject<'_>) -> jni::errors::Result<u64> {
    let value = env.get_field(this, jni::jni_str!("handle"), jni::jni_sig!(long))?.j()?;
    Ok(value as u64)
}

/// Renders an `Object[]` of already-marshalled arguments (boxed, per
/// `marshal::box_if_primitive`'s output shape) back into the S-expression
/// list fragment `protocol::invoke_message` embeds — the reverse
/// direction of what `handle_call` (Task 5) does for its own args, reused
/// here since a JVM-initiated call's arguments need the exact same
/// Java-object -> LispArg -> sexp pipeline as any other return value.
fn jobject_array_to_sexp(env: &mut jni::Env<'_>, args: &jni::objects::JObjectArray<'_>) -> jni::errors::Result<String> {
    let len = env.get_array_length(args)?;
    let mut parts = Vec::with_capacity(len as usize);
    for i in 0..len {
        let obj = env.get_object_array_element(args, i)?;
        let arg = crate::marshal::jvm_value_to_lisp_arg(env, jni::objects::JValueOwned::Object(obj))?;
        parts.push(crate::marshal::lisp_arg_to_sexp(&arg));
    }
    Ok(format!("({})", parts.join(" ")))
}

fn register_lisp_function_native(env: &mut jni::Env) -> jni::errors::Result<()> {
    let class = env.find_class(jni::jni_str!("emacs_jni/LispFunction"))?;
    // VERIFY: exact registration call/struct shape — see this function's
    // doc comment above and the top-of-plan API-uncertainty note.
    env.register_native_methods(
        &class,
        &[jni::NativeMethod {
            name: "call".into(),
            sig: "([Ljava/lang/Object;)Ljava/util/concurrent/CompletableFuture;".into(),
            fn_ptr: lisp_function_call as *mut std::ffi::c_void,
        }],
    )
}
```

- [ ] **Step 3: Handle `Cmd::InvokeReply` for real in `src/jvm_thread.rs`**

Replace the `Cmd::InvokeReply { .. }` catch-all arm (from Task 5's
`match`) with:
```rust
            Cmd::InvokeReply { id, ok, result } => handle_invoke_reply(&vm, id, ok, result),
```

Add the handler:
```rust
/// Resolves the pending `CompletableFuture` for ID — inline, cheaply, on
/// the worker thread's own persistent JNI attachment, per spec "Worker
/// thread role": unlike Cmd::Call this never spawns a thread, since
/// completing a future is not itself a slow operation.
fn handle_invoke_reply(vm: &JavaVM, id: u64, ok: bool, result: crate::commands::LispArg) {
    let Some(future) = crate::pending::take(id) else {
        channel::send(&protocol::log_message("error", &format!("invoke-reply for unknown id {id}")));
        return;
    };
    let _: jni::errors::Result<()> = vm.attach_current_thread(|env| {
        let future_local = env.new_local_ref(future.as_obj())?;
        if ok {
            let param_type = env.find_class(jni::jni_str!("java/lang/Object"))?;
            let jvalue = crate::marshal::lisp_arg_to_jvalue(env, &result, &param_type)?;
            let boxed = crate::marshal::box_if_primitive(env, jvalue)?;
            env.call_method(
                &future_local,
                jni::jni_str!("complete"),
                jni::jni_sig!((v: java.lang.Object) -> boolean),
                &[jni::objects::JValueOwned::Object(boxed).borrow()],
            )?;
        } else {
            let crate::commands::LispArg::Str(message) = result else {
                return Ok(()); // malformed err payload; nothing sensible to do
            };
            let lisp_error_class = env.find_class(jni::jni_str!("emacs_jni/LispError"))?;
            let jmessage = env.new_string(&message)?;
            let error = env.new_object(
                &lisp_error_class,
                jni::jni_sig!((message: java.lang.String) -> void),
                &[jni::objects::JValueOwned::Object(jmessage.into()).borrow()],
            )?;
            env.call_method(
                &future_local,
                jni::jni_str!("completeExceptionally"),
                jni::jni_sig!((ex: java.lang.Throwable) -> boolean),
                &[jni::objects::JValueOwned::Object(error).borrow()],
            )?;
        }
        Ok(())
    });
}
```

- [ ] **Step 4: Run `cargo build`**

Run: `cargo build`
Expected: succeeds — but this step is the second-highest API-uncertainty
point in the plan (native method registration), after Task 1's
`DefineClass`. Budget real iteration time; consult `cargo doc -p jni
--open` liberally.

- [ ] **Step 5: Extend `elisp/emacs-jni.el`'s dispatch for `invoke`**

Add a `jvm--invoke-reply` Rust primitive first, in `src/lib.rs`:
```rust
#[defun(mod_in_name = false, name = "jvm--invoke-reply")]
fn jvm_invoke_reply(id: u64, ok: bool, result: Value) -> Result<()> {
    let arg = if ok {
        lisp_value_to_arg_no_callback(result)?
    } else {
        let message: String = result.into_rust()?;
        commands::LispArg::Str(message)
    };
    let _ = worker::send_command(commands::Cmd::InvokeReply { id, ok, result: arg });
    Ok(())
}

/// Same scalar cases as `lisp_value_to_arg` (Task 6) minus the
/// function-wrapping case — a callback's *own* return value being itself
/// another bare Lisp function is out of scope for this plan (nothing in
/// the spec's marshalling table requires it, and it would need the same
/// `env`-threading `lisp_value_to_arg` already does, for a case this plan
/// doesn't need to support yet).
fn lisp_value_to_arg_no_callback(v: Value) -> Result<commands::LispArg> {
    if v.is_not_nil() == false {
        return Ok(commands::LispArg::Nil);
    }
    if let Ok(n) = v.into_rust::<i64>() {
        return Ok(commands::LispArg::Int(n));
    }
    if let Ok(f) = v.into_rust::<f64>() {
        return Ok(commands::LispArg::Float(f));
    }
    if let Ok(s) = v.into_rust::<String>() {
        return Ok(commands::LispArg::Str(s));
    }
    if let Ok(h) = v.into_rust::<handles::JvmHandle>() {
        return Ok(commands::LispArg::Object(h.0));
    }
    Ok(commands::LispArg::True)
}
```

In `elisp/emacs-jni.el`, add a `jvm--parse-invoke-args` helper (the wire
form's `ARGS` is a plain S-expression list already — no `(jvm-handle N)`
tags expected among a JVM-initiated call's arguments in this plan's
scope, since those would only arise from a Lisp value being passed back
in, which Task 6/7 don't produce on the Java-calling-Lisp side) and extend
`jvm--dispatch-message`'s `pcase`:
```elisp
    (`(invoke ,id ,handle ,args)
     (let ((fn (gethash handle jvm--lisp-callbacks)))
       (if (null fn)
           (jvm--invoke-reply id nil (format "no such callback handle %s" handle))
         (condition-case err
             (jvm--invoke-reply id t (apply fn args))
           (error
            (jvm--invoke-reply id nil (format "%S" err)))))))
```

- [ ] **Step 6: Write `test/jvm-invoke-test.el`**

This is the plan's first genuinely end-to-end callback round trip. Add a
fixture method to `test/fixtures/Calc.java`:
```java
    public static Object callWithArg(emacs_jni.LispCallable fn, long n) throws Exception {
        // A synchronous-looking wait is fine INSIDE A TEST FIXTURE (not
        // inside the shim or any real interop path) — this just proves
        // the round trip completes with the right value; production Java
        // code calling a LispCallable would normally chain
        // .thenAccept(...) instead of blocking.
        return fn.call(n).get(10, java.util.concurrent.TimeUnit.SECONDS);
    }
```

`test/jvm-invoke-test.el`:
```elisp
;;; jvm-invoke-test.el --- Tests for the JVM-calls-Lisp round trip -*- lexical-binding: t; -*-

(require 'ert)
(require 'emacs-jni)

(defvar jvm-invoke-test--calc-jar
  (expand-file-name "../target/test-fixtures/calc.jar"
                     (file-name-directory (or load-file-name buffer-file-name))))

(ert-deftest emacs-jni-jvm-invoke-test/java-calls-lisp-and-gets-the-result-back ()
  (should (eq (jvm-start-sync) 'ready))
  (should (equal (jvm-load-jar-sync jvm-invoke-test--calc-jar) '(t)))
  (should (equal (jvm-call-sync "Calc" "callWithArg" (list (lambda (n) (* n 2)) 21))
                  '(t 42 nil))))

(ert-deftest emacs-jni-jvm-invoke-test/lisp-error-propagates-as-java-exception ()
  (should (eq (jvm-status) 'ready))
  (let ((result (jvm-call-sync "Calc" "callWithArg"
                                 (list (lambda (_n) (error "boom")) 1))))
    (should (eq (nth 0 result) nil))
    (should (string-match-p "boom" (nth 2 result)))))

(provide 'jvm-invoke-test)
;;; jvm-invoke-test.el ends here
```

- [ ] **Step 7: Run it**

Run: `./test/run-ert-tests.sh test/jvm-invoke-test.el`
Expected: passes. This is the plan's central integration point — if it
hangs rather than failing cleanly, suspect the native `call` registration
(Step 2) or the `CompletableFuture` completion call (Step 3) first.

- [ ] **Step 8: Commit**

```bash
git add src/pending.rs src/jvm_thread.rs src/shim.rs src/lib.rs elisp/emacs-jni.el test/fixtures/Calc.java test/jvm-invoke-test.el
git commit -m "Add invoke/InvokeReply: JVM calls a Lisp function and gets the result back"
```

---

## Task 8: `release`/`ReleaseRef` — GC notification in both directions

**Files:**
- Modify: `src/jvm_thread.rs` (real `Cmd::ReleaseRef` handling, replacing
  Task 2's catch-all)
- Modify: `src/shim.rs` (a `Cleaner`-based finalizer on `LispFunction` that
  posts `(release HANDLE)`)
- Modify: `elisp/emacs-jni.el` (dispatch `release`, removing the table
  entry)

**Interfaces:**
- Consumes: `handles::release` (Task 3, already exists — this task is
  what finally makes `Cmd::ReleaseRef` actually call it, completing the
  loop Task 3's `JvmHandle::Drop` started).

- [ ] **Step 1: Handle `Cmd::ReleaseRef` for real**

In `src/jvm_thread.rs`, replace the `Cmd::ReleaseRef { .. }` catch-all arm
with:
```rust
            Cmd::ReleaseRef { handle } => crate::handles::release(handle),
```

This one is trivial (Task 3's `handles::release` already exists and is
cheap) — no thread spawn needed, matches spec "Worker thread role":
"`Cmd::ReleaseRef` is handled inline (a `GlobalRef` drop is cheap)."

- [ ] **Step 2: Add a `Cleaner`-based finalizer to `LispFunction`, notifying Lisp when the JVM-side proxy is collected**

This is the JVM-side mirror of Task 3's `JvmHandle::Drop` — it needs a
Java-level `java.lang.ref.Cleaner` registered when a `LispFunction` is
constructed, whose cleanup action calls back into Rust to send `(release
HANDLE)`. Registering the cleanup action itself needs another native
method (or: do the registration from Rust, right after constructing the
`LispFunction` in `marshal.rs`'s `Callback` arm, rather than adding
Java-side `Cleaner` code to the shim — simpler, since it avoids needing a
*second* native method and its own registration dance). Do it from Rust:

In `src/marshal.rs`, extend the `LispArg::Callback` arm (from Task 6) to
also register a cleaner action:
```rust
        (LispArg::Callback(handle) , _) => {
            let lisp_function_class = env.find_class(jni::jni_str!("emacs_jni/LispFunction"))?;
            let obj = env.new_object(
                &lisp_function_class,
                jni::jni_sig!((handle: long) -> void),
                &[JValueOwned::Long(*handle as i64).borrow()],
            )?;
            register_release_on_gc(env, &obj, *handle)?;
            Ok(JValueOwned::Object(obj))
        }
```

```rust
/// Registers a `java.lang.ref.Cleaner` action on OBJ that, when OBJ (the
/// LispFunction proxy) becomes phantom-reachable, sends `(release
/// HANDLE)` — the JVM-side half of the spec's "Identity and lifetime"
/// GC-notification loop. The cleaner runs on a JVM-managed background
/// thread at some point after OBJ is collected, not synchronously — this
/// is standard `Cleaner` behavior, not a bug.
///
/// VERIFY: `java.lang.ref.Cleaner`'s `create()`/`register(Object,
/// Runnable)` static/instance methods are plain public JDK API, so the
/// reflective calls below should be mechanical — but the *Runnable*
/// argument needs to itself be a JVM object whose `run()` calls back into
/// Rust. The shim doesn't currently define a Runnable-implementing class
/// for this. Simplest fix: add a fourth small shim class (e.g.
/// `ReleaseNotifier implements Runnable`, constructed with the handle,
/// with a native `run()` that sends the release message) alongside
/// `LispCallable`/`LispFunction`/`LispError` in Task 1's `java/emacs_jni/`
/// directory, registered into the JVM and given a native method exactly
/// like `LispFunction.call` was in Task 7. Add that class now as part of
/// this step (`java/emacs_jni/ReleaseNotifier.java`), rather than trying
/// to shoehorn the notification into an existing class.
fn register_release_on_gc(env: &mut jni::Env<'_>, obj: &JObject<'_>, handle: u64) -> jni::errors::Result<()> {
    let notifier_class = env.find_class(jni::jni_str!("emacs_jni/ReleaseNotifier"))?;
    let notifier = env.new_object(
        &notifier_class,
        jni::jni_sig!((handle: long) -> void),
        &[JValueOwned::Long(handle as i64).borrow()],
    )?;

    let cleaner_class = env.find_class(jni::jni_str!("java/lang/ref/Cleaner"))?;
    let cleaner = env
        .call_static_method(&cleaner_class, jni::jni_str!("create"), jni::jni_sig!(() -> java.lang.ref.Cleaner), &[])?
        .l()?;
    env.call_method(
        &cleaner,
        jni::jni_str!("register"),
        jni::jni_sig!((obj: java.lang.Object, action: java.lang.Runnable) -> java.lang.ref.Cleaner$Cleanable),
        &[JValueOwned::Object(obj.clone()).borrow(), JValueOwned::Object(notifier).borrow()],
    )?;
    Ok(())
}
```

Add the new shim class, `java/emacs_jni/ReleaseNotifier.java`:
```java
package emacs_jni;

final class ReleaseNotifier implements Runnable {
    private final long handle;

    ReleaseNotifier(long handle) {
        this.handle = handle;
    }

    @Override
    public native void run();
}
```

Update `build.rs` to also compile it (add
`.arg(format!("{manifest_dir}/java/emacs_jni/ReleaseNotifier.java"))` to
the `javac` invocation), and `src/shim.rs` to embed and `DefineClass` it
plus register its native `run()` method the same way `LispFunction.call`
was registered in Task 7 — the native implementation just does:
```rust
extern "system" fn release_notifier_run<'local>(mut env: jni::Env<'local>, this: jni::objects::JObject<'local>) {
    // NOTE: this is the opposite direction from Step 1's `Cmd::ReleaseRef`
    // (Lisp's user-ptr GC'd -> tell Rust to drop a GlobalRef, over the
    // Lisp->worker Cmd channel). Here the JVM-side LispFunction proxy was
    // GC'd, so Lisp needs telling (over the worker->Lisp channel) to drop
    // its function-table entry — a `(release HANDLE)` wire message via
    // `channel::send`, not a `Cmd` sent via `worker::send_command`. Do not
    // conflate the two paths.
    if let Ok(handle) = get_handle_field(&mut env, &this) {
        crate::channel::send(&crate::protocol::release_message(handle));
    }
}
```
(`get_handle_field` already exists from Task 7 — reuse it, no
duplication.)

- [ ] **Step 3: Dispatch `release` messages Lisp-side**

In `elisp/emacs-jni.el`'s `jvm--dispatch-message` `pcase`, add:
```elisp
    (`(release ,handle)
     (remhash handle jvm--lisp-callbacks))
```

- [ ] **Step 4: Run `cargo build` and the full test suite**

Run: `cargo build && cargo test --lib && ./test/run-ert-tests.sh`
Expected: everything from Tasks 1-7 still passes; this task adds no new
Lisp-visible behavior that changes any existing test's expected output
(GC timing is non-deterministic, so this task deliberately has no new
automated test asserting a `release` message actually arrives — that
would mean forcing GC in a test, which is flaky by nature; Task 9's final
review can spot-check this manually if desired, see that task's notes).

- [ ] **Step 5: Commit**

```bash
git add src/jvm_thread.rs src/marshal.rs elisp/emacs-jni.el java/emacs_jni/ReleaseNotifier.java build.rs src/shim.rs
git commit -m "Add release/ReleaseRef GC notification in both directions"
```

---

## Task 9: End-to-end verification pass

Not new functionality — this task is a deliberate final integration check
across everything Tasks 1-8 built, per the spec's "Testing" section and
this project's established subagent-driven-development practice of a
final whole-branch review after the last task (see HANDOFF.md's account of
Phase 1: two real cross-task bugs were caught this way that no single
task's own review could see).

**Files:**
- Modify: `README.md` (update the Phase 2 status/roadmap section, add a
  short interop example mirroring the existing jar-loading walkthrough)
- Modify: `HANDOFF.md` (replace with a fresh handoff note for whoever/
  whatever picks up Phase 3 — see Task structure below)

- [ ] **Step 1: Run the complete test suite from a clean build**

Run: `cargo test --lib && EMACS_JNI_TEST_FEATURES=test-support ./test/run-ert-tests.sh`
Expected: every test from every task passes together, not just
individually — this is the first point in the plan where all of them run
in the same process lifecycle back-to-back (Emacs batch mode's one-JVM-
per-process constraint, same as Phase 1, means each `test/*.el` file is
still its own process, but `run-ert-tests.sh`'s default of running every
`test/*.el` file exercises the full set in one invocation).

- [ ] **Step 2: Manual smoke test matching the design doc's intent**

Not scriptable as a single ERT assertion — this is a human/agent-in-the-
loop sanity check that the *interactive* experience described in the spec
actually works, the same spirit as the README's existing Quickstart
walkthrough. From the repo root:
```
cargo build
emacs -Q -L elisp
```
Then in `*scratch*`:
```elisp
(require 'emacs-jni)
(jvm-start-sync)
(jvm-load-jar-sync (expand-file-name "target/test-fixtures/calc.jar"))
(jvm-call-sync "Calc" "addLong" '(2 3))
;; => (t 5 nil)
(jvm-call-sync "Calc" "callWithArg" (list (lambda (n) (* n 10)) 4))
;; => (t 40 nil)
```
Confirm this matches expectations before moving on. If it doesn't, that's
a real finding — don't paper over it by adjusting only the automated
tests.

- [ ] **Step 3: Update `README.md`**

Move Phase 2 from "not started" to "done" in the Roadmap section, and add
a short interop example (mirroring the existing jar-loading Quickstart
section's style) using the exact commands verified in Step 2.

- [ ] **Step 4: Write a fresh `HANDOFF.md`**

Following the structure of the existing one (which this replaces) —
"Where things stand" (Phase 2 done, summarize what shipped and any
deviations from the spec found necessary during implementation, the same
way Phase 1's handoff documented its `jni` `OnceLock` finding),  "Known
rough edges" (at minimum: overload resolution picks the first arg-count
match rather than doing real assignability scoring — see Task 5, Step 2's
note; no automated test for the GC-notification path — see Task 8's
note), and "What's next" pointing at Phase 3 (Clojure eval + nREPL,
not yet brainstormed — see this repo's own process for how Phase 2 itself
got started).

- [ ] **Step 5: Commit**

```bash
git add README.md HANDOFF.md
git commit -m "Phase 2 complete: update README and HANDOFF for Phase 3"
```
