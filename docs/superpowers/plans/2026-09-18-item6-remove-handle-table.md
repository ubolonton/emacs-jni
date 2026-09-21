# Item 6 — Remove the JVM-Object Handle Table Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `u64` → `Arc<Global<JObject>>` handle table with a live reference carried directly in the value enums and the Lisp `user-ptr`, deleting `src/handles.rs`'s table.

**Architecture:** A new `JvmObject(Arc<Global<JObject<'static>>>)` newtype (in `handles.rs`, so `commands.rs` stays `jni`-free) replaces the `u64` in `BridgeValue::Object`, `CallTarget::Object`, `Cmd::ReleaseRef`, and `JvmHandle`. Producers mint the `Global` and wrap it; consumers deref it directly; `JvmHandle::Drop` clones the `Arc` to the worker so `DeleteGlobalRef` runs on the JNI-attached worker thread. The process-global `Mutex<HashMap>` table plus `register`/`resolve`/`release` go away. `jni::refs::Global` is `Send + Sync`, so the reference rides the typed `Cmd`/`Inbound` queues unchanged — the JVM-object mirror of what Item 2 already proved for the Lisp direction.

**Tech Stack:** Rust (`jni` 0.22.4, `emacs` 0.21), Emacs Lisp / ERT.

**Spec:** `docs/superpowers/specs/2026-09-18-marshalling-consolidation-design.md` (§ "Item 6", § "Decisions").

## Global Constraints

- Java 26 via `mise`; `jni` pinned 0.22.4; `emacs` 0.21 (`emacs-28` feature); Linux / Emacs 28+ only.
- `cargo fmt` and `cargo clippy --all-targets` clean before each commit (both are no-op-clean on `main`; a new warning is yours).
- NEVER build with the release profile. PREFER running specific tests over the whole suite.
- AVOID `panic!`/`unreachable!`/`.unwrap()`/`unsafe`/clippy `allow`; PREFER `if let`, let-chains, top-level imports, full variable names.
- Commit prose in Simplified Technical English, minimal words. Each commit self-contained with its own tests.
- NEVER commit files under `docs/superpowers/handoffs/`.
- Preserve the `JvmHandle` single-owner property (mirror of `LispObject`): exactly one wrapper owns a given reference, released exactly once, on a JNI-attached thread. Each JVM→Lisp crossing mints a fresh `Global`.
- Decision (confirmed with the user): carry the reference behind the `JvmObject` newtype in `handles.rs`; keep `commands.rs` free of direct `jni` imports. Item 7 is deferred (out of scope here).

## File Structure

- `src/handles.rs` — becomes `JvmObject` newtype + `JvmHandle` (`Drop`, `Transfer`). Table + `register`/`resolve`/`release`/`NEXT_HANDLE` + its `#[cfg(test)] mod tests` deleted.
- `src/commands.rs` — `BridgeValue::Object`, `CallTarget::Object`, `Cmd::ReleaseRef` carry `JvmObject` (imported from `handles`). Stays free of direct `jni` imports.
- `src/marshal.rs` — object producer (`object_to_lisp_arg`) mints the ref; consumer (`lisp_arg_to_jvalue`) derefs it. No `handles::register`/`resolve`.
- `src/jvm_thread.rs` — `CallTarget::Object` derefs the ref; `Cmd::ReleaseRef` drops it (release on the attached worker thread).
- `src/lisp_requests.rs` — `bridge_value_to_value` builds the `JvmHandle` `user-ptr` directly (no `jvm--wrap-handle`).
- `src/lib.rs` — `parse_call_target` / `lisp_value_to_arg` clone the ref out of a `&JvmHandle`; delete the `jvm--wrap-handle` and `jvm--test-make-handle` defuns.
- `test/jvm-call-test.el` — add a real-object distinctness test; `test/jvm-handle-test.el` deleted.
- `docs/explanation/interop-design.md` — sync the object-model note (JVM-object direction no longer uses a Rust-side table).

---

### Task 1: Supersede and remove the obsolete dummy-handle scaffolding

`jvm--test-make-handle` / `handles::register_dummy_for_test` / `test/jvm-handle-test.el` are plumbing-only scaffolding from before real method calls produced handles (see the doc comment on `jvm_test_make_handle`, `src/lib.rs`). They mint a fake `u64` with no live JVM object — impossible once `JvmHandle` holds a live reference (Task 2). Real object round-trips (`static-call-and-instance-call`, `jvm-call-test.el` / `jvm-call-direct-test.el`) already cover the `user-ptr` plumbing; this task adds the one property the dummy test uniquely checked (distinct handles are not `eq`) against real objects, then removes the scaffolding. This is a refactor: the new test is green on current code (characterization), locking the property before deletion.

**Files:**
- Modify: `test/jvm-call-test.el`
- Delete: `test/jvm-handle-test.el`
- Modify: `src/lib.rs` (delete `jvm_test_make_handle`, ~`:387-401`)
- Modify: `src/handles.rs` (delete `register_dummy_for_test`, `:82-90`)

**Interfaces:**
- Consumes: `jvm-new-await`, `jvm-call-await` (existing), `Calc` fixture (constructor takes an `int` base; `addToBase(n)` returns `base + n`).
- Produces: nothing for later tasks (pure removal + one test).

- [ ] **Step 1: Add the real-object distinctness test**

Append to `test/jvm-call-test.el` (before the `(provide ...)`):

```elisp
(ert-deftest emacs-jni-jvm-call-test/distinct-objects-are-distinct-user-ptrs ()
  "Two separately constructed JVM objects wrap as distinct, non-`eq'
`user-ptr' values, each round-tripping independently. Supersedes the
plumbing-only `jvm--test-make-handle' check now that real method calls
produce genuine handles."
  (jvm-call-test--ensure-ready)
  (let ((a (jvm-new-await "Calc" '(1)))
        (b (jvm-new-await "Calc" '(2))))
    (should (user-ptrp a))
    (should (user-ptrp b))
    (should-not (eq a b))
    (should (equal (jvm-call-await a "addToBase" '(10)) 11))
    (should (equal (jvm-call-await b "addToBase" '(10)) 12))))
```

- [ ] **Step 2: Run it to confirm it passes on current code (characterization baseline)**

Run: `./test/run-ert-tests.sh test/jvm-call-test.el`
Expected: PASS, including `distinct-objects-are-distinct-user-ptrs`.

- [ ] **Step 3: Delete the dummy-handle scaffolding**

- Delete `test/jvm-handle-test.el` entirely.
- In `src/lib.rs`, delete the whole `jvm_test_make_handle` function and its `#[cfg(feature = "test-support")]` + `#[defun(... name = "jvm--test-make-handle")]` attributes and doc comment (~`:387-401`).
- In `src/handles.rs`, delete `register_dummy_for_test` and its doc comment (`:82-90`).

Do NOT touch the `test-support` feature itself — `jvm--test-panic` (crash test) still needs it.

- [ ] **Step 4: Verify the build and suite stay green**

Run: `cargo build && cargo clippy --all-targets`
Expected: clean (no unused-item warnings — `NEXT_HANDLE`/`register`/`resolve`/`release` still exist and are used by the table until Task 2).

Run: `./test/run-ert-tests.sh test/jvm-call-test.el`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add test/jvm-call-test.el src/lib.rs src/handles.rs
git rm test/jvm-handle-test.el
git commit -m "test: drop obsolete dummy-handle plumbing test

Real object round-trips cover the user-ptr path; assert distinct handles
on real objects. Prereq for Item 6 (JvmHandle will hold a live ref)."
```

---

### Task 2: Replace the handle table with a live `JvmObject` reference

The core of Item 6. A compilation-coupled refactor: the payload type of `BridgeValue::Object`, `CallTarget::Object`, `Cmd::ReleaseRef`, and `JvmHandle` all change together, so this is one atomic commit. No behaviour change — the existing object round-trip tests (async + direct) are the gate; establish them green first, refactor, confirm green.

**Files:**
- Modify: `src/handles.rs` (rewrite)
- Modify: `src/commands.rs` (`:1-9`, `:27`, `:98-100`)
- Modify: `src/marshal.rs` (`:1-4` imports, `:78-84`, `:323-345`, `:420-422`)
- Modify: `src/jvm_thread.rs` (`:74-87`, `:837-842`)
- Modify: `src/lisp_requests.rs` (`:219-222`)
- Modify: `src/lib.rs` (`:151-152`, `:212-221`, `:329-330`)
- Modify: `docs/explanation/interop-design.md` (`:170-174`)
- Test: `test/jvm-call-test.el`, `test/jvm-call-direct-test.el` (existing, unchanged — the regression gate)

**Interfaces:**
- Produces:
  - `handles::JvmObject(pub Arc<Global<JObject<'static>>>)` — `#[derive(Debug, Clone)]`.
  - `handles::JvmHandle(pub JvmObject)` — `Transfer` + `Drop`.
  - `commands::BridgeValue::Object(JvmObject)`, `commands::CallTarget::Object(JvmObject)`, `commands::Cmd::ReleaseRef { object: JvmObject }`.
- Consumes: `jni::Env::{new_global_ref, new_local_ref, get_object_class}`; `Global::as_obj`; `emacs::IntoLisp` for `Box<JvmHandle>`; `worker::send_command`.

- [ ] **Step 1: Establish the green baseline (characterization)**

Run: `./test/run-ert-tests.sh test/jvm-call-test.el test/jvm-call-direct-test.el`
Expected: PASS. `static-call-and-instance-call` (both files) returns a JVM object then passes it back — the async + direct object round trip. This is the invariant Task 2 must preserve.

Run: `cargo test`
Expected: PASS (`handles::tests` register/resolve round-trip still present here — it is deleted in Step 2).

- [ ] **Step 2: Rewrite `src/handles.rs` — newtype + wrapper, table deleted**

Replace the entire file contents (imports, `NEXT_HANDLE`, `TABLE`, `table`, `register`, `resolve`, `release`, and the `#[cfg(test)] mod tests`) with:

```rust
use std::sync::Arc;

use jni::objects::JObject;
use jni::refs::Global;

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
        // reference is handed to the worker thread to release: dropping the
        // last `Arc` there runs `Global::Drop`'s `DeleteGlobalRef` on an
        // attached thread. `jni::Global::Drop` would otherwise auto-attach
        // the GC thread with a logged perf warning (jni-0.22.4,
        // `src/refs/global.rs`). Cloning (not moving) the `Arc` keeps a
        // reference alive past this frame, so the field's own drop below
        // never triggers `DeleteGlobalRef` on this (GC) thread.
        let object = self.0.clone();
        let _ = crate::worker::send_command(crate::commands::Cmd::ReleaseRef { object });
    }
}
```

- [ ] **Step 3: Update `src/commands.rs` — enums carry `JvmObject`**

Add the import at the top of the file (keep `commands.rs` free of direct `jni` imports):

```rust
use crate::handles::JvmObject;
```

`CallTarget::Object` (`:8`):

```rust
    /// A previously wrapped JVM object, by its live shared reference.
    Object(JvmObject),
```

`BridgeValue::Object` (`:26-27`):

```rust
    /// A wrapped JVM object, by its live shared reference (Item 6 — no
    /// handle table).
    Object(JvmObject),
```

`Cmd::ReleaseRef` (`:98-100`):

```rust
    /// A Lisp `user-ptr` wrapping a JVM object was GC'd; drop its reference
    /// on the attached worker thread (see `handles::JvmHandle::Drop`).
    ReleaseRef {
        object: JvmObject,
    },
```

- [ ] **Step 4: Update `src/marshal.rs` — producer + consumer use the ref directly**

Replace the imports (`:1-4`):

```rust
use std::sync::Arc;

use jni::objects::{JClass, JObject, JValueOwned};

use crate::commands::BridgeValue;
use crate::handles::JvmObject;
```

Consumer — `lisp_arg_to_jvalue` Object arm (`:78-84`):

```rust
        (BridgeValue::Object(object), _) => {
            let local = env.new_local_ref(object.0.as_obj())?;
            Ok(JValueOwned::Object(local))
        }
```

Producer — `object_to_lisp_arg` tail (`:420-422`):

```rust
    let global = env.new_global_ref(&obj)?;
    Ok(BridgeValue::Object(JvmObject(Arc::new(global))))
```

Update the `object_to_lisp_arg` doc comment (`:323-328`): replace "get wrapped via `handles::register`" with "are wrapped as a fresh `Global` reference carried directly (Item 6 — no handle table)".

- [ ] **Step 5: Update `src/jvm_thread.rs` — deref target, release on drop**

`Cmd::ReleaseRef` arm (`:84-87`) and its doc comment (`:74-83`):

```rust
            // Handled inline, like `Cmd::InvokeReply` — dropping a
            // `GlobalRef` is one cheap JNI call (`DeleteGlobalRef`), not an
            // arbitrary user method (spec "Worker thread role"). Lisp's GC
            // collected a `user-ptr` wrapping a JVM object; its `Drop`
            // handed the shared reference here (its own `Drop` may run on a
            // thread with no JNI attachment). Dropping this last `Arc` runs
            // `Global::Drop`'s `DeleteGlobalRef` on this attached worker
            // thread.
            Cmd::ReleaseRef { object } => {
                std::mem::drop(object);
                false
            }
```

`CallTarget::Object` arm (`:837-842`):

```rust
        crate::commands::CallTarget::Object(object) => {
            let local = env.new_local_ref(object.0.as_obj())?;
            (env.get_object_class(&local)?, Some(local))
        }
```

- [ ] **Step 6: Update `src/lisp_requests.rs` — build the `user-ptr` directly**

`bridge_value_to_value` Object arm (`:219-222`):

```rust
        // Build the `JvmHandle` `user-ptr` directly from the shared
        // reference — no wire, no table, no `jvm--wrap-handle` round trip.
        BridgeValue::Object(object) => {
            Box::new(crate::handles::JvmHandle(object.clone())).into_lisp(env)
        }
```

(`IntoLisp` is already in scope — the scalar arms above use `.into_lisp(env)`.)

- [ ] **Step 7: Update `src/lib.rs` — clone out of `&JvmHandle`, delete `jvm--wrap-handle`**

`parse_call_target` (`:151-152`):

```rust
        let h: &handles::JvmHandle = target.into_rust()?;
        Ok(commands::CallTarget::Object(h.0.clone()))
```

`lisp_value_to_arg` Object arm (`:329-330`):

```rust
    if let Ok(h) = v.into_rust::<&handles::JvmHandle>() {
        return Ok(commands::BridgeValue::Object(h.0.clone()));
    }
```

Delete the whole `jvm_wrap_handle` function and its `#[defun(... name = "jvm--wrap-handle")]` attribute and doc comment (`:212-221`) — its only caller was `bridge_value_to_value` (now builds the `user-ptr` in Rust), and no Elisp references it (verified: `grep -rn wrap-handle elisp/` is empty).

- [ ] **Step 8: Build, lint, format**

Run: `cargo build && cargo clippy --all-targets && cargo fmt`
Expected: compiles clean, no warnings, `fmt` a no-op on unchanged lines. If clippy flags `std::mem::drop` in Step 5 (`drop_non_drop`), it is a false alarm only if `Arc` were `Copy` — it is not; leave `std::mem::drop(object)` as the documented release point.

- [ ] **Step 9: Run the regression gate**

Run: `cargo test`
Expected: PASS (the deleted `handles::tests` no longer exist; `marshal::jvm_tests` still pass).

Run: `./test/run-ert-tests.sh test/jvm-call-test.el test/jvm-call-direct-test.el`
Expected: PASS — object round trip preserved on both the async (worker-crossing) and direct paths, including the new distinctness test from Task 1.

Run (broader, needs Clojure jars — the callback/symbol/clj paths also carry `BridgeValue::Object`):
`EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars" ./test/run-ert-tests.sh test/jvm-invoke-test.el test/jvm-symbol-marshal-test.el test/jvm-clj-test.el test/jvm-clj-direct-test.el`
Expected: PASS. (If the jars are absent, run `test/fetch-clojure-jars.sh` first.)

- [ ] **Step 10: Sync `docs/explanation/interop-design.md`**

The object-model note contrasts the Lisp-object direction with the JVM-object direction, implying the latter keeps a Rust-side table (line ~174: "There is no Rust-side handle table for a `LispObject`"). After Item 6 neither direction uses one. Replace the sentence at `:170-172`:

Old:
```
This mirrors the other object direction — a JVM object returned to Lisp,
which Lisp likewise holds as an opaque `user-ptr` token for a reference
only the JVM side can use.
```

New:
```
This mirrors the other object direction — a JVM object returned to Lisp,
which Lisp holds as an opaque `user-ptr` wrapping an `Arc`-shared `Global`
reference only the JVM side can use. Neither direction keeps a Rust-side
lookup table: each token owns its reference directly. They differ only in
what the token holds — a boxed `GlobalRef` pointer for a `LispObject`, an
`Arc<Global>` for a JVM object — and which thread frees it: the Emacs main
thread for a `LispObject`, the JVM worker thread for a JVM object.
```

- [ ] **Step 11: Commit**

```bash
git add src/handles.rs src/commands.rs src/marshal.rs src/jvm_thread.rs \
        src/lisp_requests.rs src/lib.rs docs/explanation/interop-design.md
git commit -m "refactor: carry JVM objects by live ref, drop handle table

Item 6. BridgeValue/CallTarget/Cmd::ReleaseRef and JvmHandle hold a
JvmObject (Arc<Global>) instead of a u64 into a process-global table.
Global is Send+Sync, so the ref rides the typed queues directly; release
stays on the attached worker thread. Deletes handles::{TABLE,register,
resolve,release} and jvm--wrap-handle."
```

---

## Self-Review

**Spec coverage (§ Item 6):**
- "carry `Arc<Global<JObject>>` in `BridgeValue::Object` and box it straight into the `JvmHandle` `user-ptr`" → Steps 2–7 (via the `JvmObject` newtype, per the confirmed decision).
- "deleting `handles::{TABLE,register,resolve,release}`" → Step 2.
- Entry points listed in the handoff — `commands.rs:27` (Step 3), `handles.rs` + `JvmHandle::Drop` (Step 2), `marshal.rs:10`/`:346` (Step 4), `lisp_requests.rs:165` + `lib.rs` `JvmHandle` construction (Steps 6–7), worker `Cmd::ReleaseRef` (Step 5) — all covered.
- Caveat "confirm the `jni` coupling" → resolved (newtype in `handles.rs`; `commands.rs` stays free of direct `jni` imports).
- Caveat "keep releasing on the worker thread" → Steps 2 (Drop clones to worker) + 5 (worker drops).
- "Do NOT touch the Lisp-object `long`" → untouched (no edits to `box_value`/`borrow`/`LispObject`).
- Preserve `LispObject`/`JvmHandle` single-owner property → `JvmHandle` remains a single-owner wrapper, no clone/copy; each crossing mints a fresh `Global` (Step 4 producer).
- Tests: object round-trip (async + direct) → Step 9; release-on-GC → manual, per the documented GC-notification gap (unchanged).
- Item 7 → deferred (out of scope, per the user's decision).

**Type consistency:** `JvmObject` field is `.0` (an `Arc<Global<JObject<'static>>>`) everywhere: `.0.as_obj()` for JNI (Steps 4, 5), `.0.clone()` / `object.clone()` for a cheap `Arc` bump (Steps 2, 6, 7). `Cmd::ReleaseRef` field is named `object` at every site (Steps 2, 3, 5).

**Placeholder scan:** none — every step carries concrete code or an exact command.
