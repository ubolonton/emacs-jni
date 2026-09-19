# `src/` Module Reorganization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restructure `src/` from 13 flat files into a `lisp/`+`jvm/`+`protocol.rs` tree where every file's role (entry / convert / inbox / runtime) is symmetric across both sides and self-evident from its path. Pure move-and-rename: no behavior change, no test file changes.

**Architecture:** Two side modules (`lisp/`, `jvm/`), each a thin manifest root (`lisp.rs`, `jvm.rs`, `mod` declarations only) plus four leaf files: `entry.rs` (the other runtime calling in), `convert.rs` (this side's value ↔ shared-protocol conversion), `inbox.rs` (async request/reply: owns the message envelope, the drain loop, and the submit function that enqueues *and* wakes the receiver in one call), `runtime.rs` (the synchronous handle the *other* side's direct-call path reaches into). One file, `protocol.rs`, sits outside both sides holding the payload types both inboxes carry (`BridgeValue`, `CallTarget`, `LispOutcome`, `LispInput`, `Status`) — never the envelope types (`Cmd`, `Inbound`), which belong to their receiving side.

**Tech Stack:** Rust (`jni` 0.22.4, `emacs` 0.21.0), Emacs Lisp / ERT.

**Spec:** `docs/superpowers/specs/2026-09-19-src-module-reorg-design.md`

## Global Constraints

- Pure reorganization. If a step requires changing anything beyond a file location, an item's name, or an import path, stop and flag it — it doesn't belong in this plan.
- `cargo clippy --all-targets` and `cargo fmt` clean before every commit (rust.md).
- NEVER build with the release profile. PREFER running specific/targeted tests over the whole suite while iterating; run the full suite (last step of every task) before committing.
- AVOID `panic!`/`unreachable!`/`.unwrap()`/`unsafe`/clippy `allow` — none of the moved code should need new instances of these; if `cargo clippy` flags one that wasn't there before, that's a sign something was mis-moved, not something to suppress.
- No test file (`test/*.el`, `src/**/#[cfg(test)]` modules) changes except updating a `use`/module path inside an existing `#[cfg(test)] mod tests` block that moves along with its parent file.
- Commit messages: conventional commits, type `refactor`, scope `rust` (e.g. `refactor(rust): Move BridgeValue/CallTarget/LispOutcome into protocol.rs`). One commit per task.
- **The fix-up procedure, used in every task below:** after moving/renaming the named items, run `cargo build`. It will report every remaining reference to the old path — update each to the new path per that task's rename table, and re-run until it succeeds with zero errors and zero new warnings. Do not hand-search for stragglers; the compiler enumerates them exhaustively. A task is not done until `cargo build` is clean.
- Known trap: several old files are emptied across *multiple* tasks (e.g. `lisp_requests.rs` across Tasks 1, 7, 9). Only touch the items a given task's mapping row assigns to it — leave every other reference in that file pointing at wherever its target *currently* lives (old path or new), even if that's not its final home yet. The task that moves that target is the one that updates references to it.

## Baseline (verified during planning)

`cargo test --lib`: 22 passed, 0 failed. Full ERT suite
(`EMACS_JNI_CLOJURE_JARS_DIR=$(pwd)/target/clojure-jars EMACS_JNI_TEST_FEATURES=test-support ./test/run-ert-tests.sh`,
no arguments): 16 test files, 83 tests, 0 unexpected. Every "same as baseline" callout below means these two numbers.

## File Structure (target state)

```text
src/
├── lib.rs              # mod declarations + module init glue only
├── protocol.rs          # BridgeValue, CallTarget, LispOutcome, LispInput, Status
├── lisp.rs              # mod entry; mod convert; mod inbox; mod runtime;
├── lisp/
│   ├── entry.rs          # the #[defun]s
│   ├── convert.rs        # Lisp Value <-> BridgeValue/LispInput/LispOutcome
│   ├── inbox.rs          # Inbound + submit() (enqueue+poke) + drain loop
│   └── runtime.rs        # env-stack for jvm/'s reentrant direct-path callbacks
├── jvm.rs                # mod entry; mod convert; mod inbox; mod invoke; mod runtime;
└── jvm/
    ├── entry.rs           # the Lisp.* JNI natives
    ├── convert.rs         # BridgeValue/LispInput <-> JValue/JObject, + JvmObject/JvmHandle
    ├── inbox.rs           # Cmd + submit() (send_command) + worker loop + lifecycle + pending
    ├── invoke.rs           # call machinery shared by async and direct
    └── runtime.rs           # JavaVM + classloader handle + JVM status (StatusCell)
```

---

### Task 1: Extract `protocol.rs`

Pulls every payload type that crosses the Lisp/JVM boundary out of `commands.rs`, `lisp_requests.rs`, and `status.rs` into one shared, `jni`-free file. This unblocks every later task, since almost everything references at least one of these types.

**Files:**
- Create: `src/protocol.rs`
- Modify: `src/commands.rs` (shrinks to just `Cmd` — deleted fully in Task 4)
- Modify: `src/lisp_requests.rs` (loses `LispInput`)
- Modify: `src/status.rs` (loses `Status`/`as_tag` — shrinks to just `StatusCell`/`STATUS`, deleted fully in Task 3)
- Modify: `src/lib.rs` (mod declarations, `use` fixups)

**Interfaces:**
- Produces: `crate::protocol::{BridgeValue, CallTarget, LispOutcome, LispInput, Status}` — every later task imports from here instead of `commands`/`status`.

- [ ] **Step 1: Create `protocol.rs` with the four payload types**

Move, verbatim (keep all doc comments — they're accurate for the new location too):
- `CallTarget`, `BridgeValue`, `LispOutcome` from `src/commands.rs:4-116` (everything except the `Cmd` enum, `:72-116`, which stays in `commands.rs` for now).
- `LispInput` from `src/lisp_requests.rs:42-56`.
- `Status`, `impl Status { fn from_u8, pub fn as_tag }` from `src/status.rs:1-49`.

`protocol.rs` needs `use std::sync::atomic::{AtomicU8, Ordering};`? No — that's only used by `StatusCell`, which stays in `status.rs` for now. `protocol.rs`'s only cross-module need is `use crate::jvm::convert::JvmObject;` for `BridgeValue::Object`/`CallTarget::Object` — but `JvmObject` doesn't move to `jvm/convert.rs` until Task 2, so for now keep `use crate::handles::JvmObject;` (unchanged target, will be fixed in Task 2).

- [ ] **Step 2: Add `mod protocol;` to `lib.rs`, delete the moved items from their old files**

`src/commands.rs` keeps only the `Cmd` enum and its doc comment. `src/lisp_requests.rs` keeps `Inbound` and everything else, minus `LispInput`; add `use crate::protocol::LispInput;`. `src/status.rs` keeps `StatusCell`, `STATUS`, and the `#[cfg(test)] mod tests` (update the tests' `Status::` references to come from `use crate::protocol::Status;` at the top of `status.rs`).

- [ ] **Step 3: Fix up every cross-reference (fix-up procedure)**

Known call sites (from `grep -rn 'commands::\|status::Status'`): `src/lisp_requests.rs`, `src/jvm_thread.rs`, `src/lib.rs`, `src/marshal.rs`, `src/worker.rs`, `src/shim.rs`, `src/handles.rs`. In each, `commands::{BridgeValue,CallTarget,LispOutcome}` → `protocol::{BridgeValue,CallTarget,LispOutcome}`; `commands::Cmd` stays `commands::Cmd` (not moving yet); `status::Status` → `protocol::Status`; `status::STATUS` stays `status::STATUS` (not moving yet). Run `cargo build`, fix what it reports, repeat to green.

- [ ] **Step 4: Verify and commit**

Run: `cargo test --lib` (expect 22 passed, same as baseline) and `cargo clippy --all-targets && cargo fmt --check`.

```bash
git add -A
git commit -m "refactor(rust): Extract protocol.rs for boundary-crossing payload types"
```

---

### Task 2: Build `jvm/convert.rs`

Merges `marshal.rs`, `handles.rs`, and the value-conversion pieces of `shim.rs` into one file: everything that turns a `BridgeValue`/`LispInput` into a JNI value or back.

**Files:**
- Create: `src/jvm/convert.rs`
- Delete: `src/marshal.rs`, `src/handles.rs`
- Modify: `src/shim.rs` (loses `object_to_lisp_request_arg`, `jobject_array_to_lisp_request_args`, `jobject_array_to_bridge_values`)
- Modify: `src/protocol.rs` (fix the `JvmObject` import, per Task 1 Step 1's note)

**Interfaces:**
- Produces: `crate::jvm::convert::{from_bridge, to_bridge, to_input, JvmObject, JvmHandle}` (plus `pub(crate)` helpers: `box_if_primitive`, `marshal_error`, `jobject_array_to_lisp_request_args`, `jobject_array_to_bridge_values`).

- [ ] **Step 1: Create the directory and file, move + rename**

`mkdir -p src/jvm`. Move into `src/jvm/convert.rs`:
- `src/marshal.rs` in full (all functions: `lisp_arg_to_jvalue`, `register_release_on_gc`, `big_integer_from_str`, `class_name`, `marshal_error`, `box_if_primitive`, `box_via_value_of`, `jvm_value_to_lisp_arg`, `object_to_lisp_arg`, plus its `#[cfg(test)] mod jvm_tests`).
- `src/handles.rs` in full (`JvmObject`, `JvmHandle`, its `Drop` impl).
- `object_to_lisp_request_arg` (`src/shim.rs:701-732`) and `jobject_array_to_lisp_request_args` (`src/shim.rs:733-end of fn`).
- `jobject_array_to_bridge_values` (`src/shim.rs:322-334`) — this one converts a JVM `Object[]` to `Vec<BridgeValue>` for the generic callback-invoke path (`lisp_function_call`); it belongs here for the same reason as its `LispInput` sibling, not in `entry.rs`.

Rename while moving:
- `lisp_arg_to_jvalue` → `from_bridge`
- `jvm_value_to_lisp_arg` → `to_bridge` (keep its internal call to the old `object_to_lisp_arg` — rename that inner one to `object_to_bridge`, private)
- `object_to_lisp_request_arg` → `to_input`

Delete `src/marshal.rs` and `src/handles.rs`.

- [ ] **Step 2: Fix the internal renames**

Inside the new `jvm/convert.rs`, update the one call site where `to_bridge`'s body calls the renamed `object_to_bridge` (was `object_to_lisp_arg`, called from the `JValueOwned::Object(obj) =>` arm, old `marshal.rs:336`).

- [ ] **Step 3: Fix up every cross-reference (fix-up procedure)**

Known call sites for the old names (from `grep -rn 'marshal::\|handles::'`): `src/jvm_thread.rs`, `src/lisp_requests.rs`, `src/shim.rs`, `src/lib.rs`. Replace `marshal::lisp_arg_to_jvalue` → `jvm::convert::from_bridge`, `marshal::{jvm_value_to_lisp_arg}` → `jvm::convert::to_bridge`, `marshal::box_if_primitive`/`marshal::marshal_error` → `jvm::convert::{box_if_primitive, marshal_error}`, `handles::JvmObject`/`handles::JvmHandle` → `jvm::convert::{JvmObject, JvmHandle}`, `shim::object_to_lisp_request_arg` → `jvm::convert::to_input`. Do **not** touch `JvmHandle::Drop`'s reference to `crate::commands::Cmd::ReleaseRef`/`crate::worker::send_command` — those move in Task 4. Run `cargo build`, fix what it reports, repeat to green. Add `mod convert;` to a new `src/jvm.rs`.

- [ ] **Step 4: Verify and commit**

Run: `cargo test --lib` and `cargo clippy --all-targets && cargo fmt --check`.

```bash
git add -A
git commit -m "refactor(rust): Merge marshal.rs and handles.rs into jvm/convert.rs"
```

---

### Task 3: Build `jvm/runtime.rs`

**Files:**
- Create: `src/jvm/runtime.rs`
- Delete: `src/runtime.rs`, `src/status.rs`

**Interfaces:**
- Produces: `crate::jvm::runtime::{set_vm, vm, get_or_create_loader, loader, set_main_thread, on_main_thread, main_attached, set_main_attached, clear, STATUS}` (all unchanged names) and `crate::jvm::runtime::StatusCell` (unchanged; `crate::protocol::Status` — moved in Task 1 — is what it stores).

- [ ] **Step 1: Move and merge**

Move `src/runtime.rs` in full into `src/jvm/runtime.rs`. Append `StatusCell`, `STATUS`, and the `#[cfg(test)] mod tests` from `src/status.rs` (post-Task-1 remainder) into the same file; add `use crate::protocol::Status;` at the top. Delete `src/runtime.rs` and `src/status.rs`.

- [ ] **Step 2: Fix up every cross-reference (fix-up procedure)**

Known call sites (from `grep -rn 'runtime::\|status::STATUS'`): `src/jvm_thread.rs` (all fully-qualified `crate::runtime::X`), `src/lib.rs` (bare `runtime::X` — has a `use crate::runtime;` to update to `use crate::jvm::runtime;`, and a `use crate::status::{Status, STATUS};` to update — `Status` now comes from `crate::protocol::Status`, `STATUS` from `crate::jvm::runtime::STATUS`). Update `jvm/convert.rs` too if it references `status::STATUS` anywhere (it shouldn't, but check). Run `cargo build`, fix what it reports, repeat to green. Add `mod runtime;` to `src/jvm.rs`.

- [ ] **Step 3: Verify and commit**

Run: `cargo test --lib` and `cargo clippy --all-targets && cargo fmt --check`.

```bash
git add -A
git commit -m "refactor(rust): Merge runtime.rs and status.rs into jvm/runtime.rs"
```

---

### Task 4: Build `jvm/inbox.rs`

Merges `worker.rs`, `pending.rs`, `commands.rs`'s remaining `Cmd`, and `jvm_thread.rs`'s event-loop/lifecycle half.

**Files:**
- Create: `src/jvm/inbox.rs`
- Delete: `src/worker.rs`, `src/pending.rs`, `src/commands.rs`
- Modify: `src/jvm_thread.rs` (loses everything except the reflection half; Task 5 moves the rest and deletes the file — do not delete it in this task)
- Modify: `src/jvm/convert.rs` (fix `JvmHandle::Drop`'s reference, deferred from Task 2)

**Interfaces:**
- Consumes: `crate::jvm::convert::JvmObject` (Task 2), `crate::jvm::runtime::*` (Task 3), `crate::protocol::LispOutcome` (Task 1).
- Produces: `crate::jvm::inbox::{Cmd, submit, next_request_id, start_or_resume, clear_sender, register, next_id}` (the last two, from `pending.rs`, `pub(crate)` — `jvm/entry.rs` calls them in Task 6) plus the worker thread's `run` function and lifecycle handlers, kept `pub(crate)` where `jvm/invoke.rs` (Task 5) or `lisp/entry.rs` (Task 10) needs them: `run`, `load_jar_direct`.

- [ ] **Step 1: Move and merge**

Move into `src/jvm/inbox.rs`:
- `src/commands.rs`'s remaining `Cmd` enum (`:72-116` from before Task 1's edit — by now it's the whole file). Delete `src/commands.rs`.
- `src/worker.rs` in full (`next_request_id`, `start_or_resume`, `send_command`, `clear_sender`). Delete `src/worker.rs`.
- `src/pending.rs` in full (`table`, `next_id`, `register`, `take`). Delete `src/pending.rs`.
- From `src/jvm_thread.rs`: `run` (`:16-171`), `is_first_attempt` (`:172-175`), `create_jvm` (`:176-257`), `panic_message` (`:258-267`), `handle_pause` (`:268-292`), `handle_stop` (`:293-324`), `handle_resume` (`:325-349`), `handle_load_jar` (`:350-402`), `load_jar_direct` (`:403-424`), `create_shared_classloader` (`:425-456`), `add_jar_url` (`:457-527`, plus its `#[cfg(test)]` tests if any are adjacent), `handle_invoke_reply` (`:528-562`), `handle_lisp_reply` (`:563-583`).

Rename: `send_command` → `submit`.

- [ ] **Step 2: Fix the internal rename and deferred Task-2 reference**

Every internal call to `send_command` inside the moved code (e.g. inside `JvmHandle::Drop` in `jvm/convert.rs`) becomes `crate::jvm::inbox::submit`, and its `crate::commands::Cmd::ReleaseRef` becomes `crate::jvm::inbox::Cmd::ReleaseRef`.

- [ ] **Step 3: Fix up every cross-reference (fix-up procedure)**

Known call sites for `worker::`, `pending::`, `commands::Cmd` (from the earlier greps): `src/lib.rs` (many — `worker::send_command` → `jvm::inbox::submit`, `worker::next_request_id`/`start_or_resume`/`clear_sender` → `jvm::inbox::{next_request_id, start_or_resume, clear_sender}`, `commands::Cmd::X` → `jvm::inbox::Cmd::X`), `src/shim.rs` (`pending::next_id`/`register` → `jvm::inbox::{next_id, register}`), `src/jvm_thread.rs`'s *remaining* content (Task 5's material still references `runtime::`/etc. unchanged — don't touch those; only fix references to items this task actually moved: `commands::LispOutcome` was already fixed in Task 1, skip). Run `cargo build`, fix what it reports, repeat to green. Add `mod inbox;` to `src/jvm.rs`.

- [ ] **Step 4: Verify and commit**

Run: `cargo test --lib` and `cargo clippy --all-targets && cargo fmt --check`.

```bash
git add -A
git commit -m "refactor(rust): Merge worker.rs, pending.rs, commands::Cmd, and the worker loop into jvm/inbox.rs"
```

---

### Task 5: Build `jvm/invoke.rs`

Moves the rest of `jvm_thread.rs` — the reflection call machinery shared by the async and direct paths — and deletes the now-empty file.

**Files:**
- Create: `src/jvm/invoke.rs`
- Delete: `src/jvm_thread.rs`

**Interfaces:**
- Consumes: `crate::jvm::inbox::*` (Task 4, for `run`'s spawn target — actually `run` itself moved to `inbox.rs`; `invoke.rs` is consumed *by* `inbox.rs`'s `handle_call`, not the reverse), `crate::jvm::convert::{from_bridge, to_bridge}` (Task 2), `crate::jvm::runtime::*` (Task 3), `crate::lisp::runtime::enter`/`in_scope`/`with_current_env` (not moved until Task 8 — leave `reentry::` references unchanged for now).
- Produces: `crate::jvm::invoke::{resolve_and_invoke, build_invoke_args, invoke_method, invoke_constructor, call_direct, build_lisp_reply_object, complete_future_with_reply, build_non_local_exit, throw_non_local_exit}` — `lisp/entry.rs` (Task 10) calls `call_direct`; `jvm/inbox.rs`'s `handle_call` calls `resolve_and_invoke`.

- [ ] **Step 1: Move**

Move the remainder of `src/jvm_thread.rs` into `src/jvm/invoke.rs`: `build_lisp_reply_object` (`:592-640`), `complete_future_with_reply` (`:641-682`), `build_non_local_exit` (`:683-708`), `throw_non_local_exit` (`:709-729`), `handle_call` (`:730-769`), `resolve_and_invoke` (`:770-894`), `call_direct` (`:895-945`), `resolve_class_via_loader` (`:946-991`), `describe_caught_java_exception` (`:992-1059`), `is_overload_arg_mismatch` (`:1060-1074`), `extract_non_local_exit` (`:1075-1115`), `invoke_method` (`:1116-1222`), `invoke_constructor` (`:1223-1307`), `build_invoke_args` (`:1308-1324`), plus the trailing `#[cfg(test)] mod tests` (`:1326-end`). No renames. Delete `src/jvm_thread.rs` (it should now be empty except `mod` boilerplate, if any remains from Task 4's edits — confirm nothing is left behind before deleting).

- [ ] **Step 2: Fix `jvm/inbox.rs`'s reference to the moved reflection call**

`jvm/inbox.rs`'s `handle_call` (moved in Task 4) calls `resolve_and_invoke` — that reference now needs `crate::jvm::invoke::resolve_and_invoke` (it was `jvm_thread::resolve_and_invoke`/an unqualified sibling call before; it's cross-file now that the two functions split across `inbox.rs` and `invoke.rs`).

- [ ] **Step 3: Fix up every cross-reference (fix-up procedure)**

Known call sites for `jvm_thread::` (from the earlier grep, excluding ones already resolved by Tasks 1–4's moves): `src/lib.rs` (`jvm_thread::load_jar_direct` — wait, that one lives in `jvm/inbox.rs` after Task 4, so this becomes `jvm::inbox::load_jar_direct`, not `jvm::invoke::` — double check against Task 4's list before fixing), `src/lib.rs` (`jvm_thread::call_direct` → `jvm::invoke::call_direct`, `jvm_thread::DirectError::*` → `jvm::invoke::DirectError::*`), `src/shim.rs` (`jvm_thread::throw_non_local_exit`, `jvm_thread::build_lisp_reply_object`, `jvm_thread::complete_future_with_reply` → `jvm::invoke::{...}`). Run `cargo build`, fix what it reports, repeat to green. Add `mod invoke;` to `src/jvm.rs`.

- [ ] **Step 4: Verify and commit**

Run: `cargo test --lib` and `cargo clippy --all-targets && cargo fmt --check`.

```bash
git add -A
git commit -m "refactor(rust): Move jvm_thread.rs's reflection machinery into jvm/invoke.rs"
```

---

### Task 6: Build `jvm/entry.rs` and the `jvm.rs` manifest

**Files:**
- Create: `src/jvm/entry.rs`
- Modify: `src/jvm.rs` (add `mod entry;` — should already have `mod convert; mod inbox; mod invoke; mod runtime;` from Tasks 2–5)
- Delete: `src/shim.rs`

**Interfaces:**
- Produces: nothing new for later Rust tasks (this is the JVM-facing native surface, called from Java, not from other Rust modules) — but confirm `jvm/entry.rs` still calls `crate::lisp::runtime::in_scope`/`with_current_env` correctly once Task 8 relocates `reentry.rs` (that fix happens in Task 8, not here — for now these stay as `crate::reentry::*`).

- [ ] **Step 1: Move**

Move everything left in `src/shim.rs` (after Task 2 removed `object_to_lisp_request_arg`/`jobject_array_to_lisp_request_args`/`jobject_array_to_bridge_values`) into `src/jvm/entry.rs`: `define_classes`, `register_lisp_function_native`, `lisp_function_call`, `register_release_notifier_native`, `release_notifier_run`, `get_handle_field`, `define_if_absent`, `register_lisp_native`, `register_lisp_object_native`, `register_lisp_ref_release_notifier_native`, `service_lisp_request_sync`, `service_lisp_request_direct`, `block_on_lisp_reply`, `rethrow_lisp_reply_exception`, `lisp_intern`, `lisp_type_of`, `lisp_is_not_nil`, `lisp_eq`, `lisp_apply_async`, `lisp_object_prin1_to_string`, `lisp_ref_release_notifier_run`, `unbox_boolean`, `get_ptr_field`, plus any `#[cfg(test)] mod tests`. No renames. Delete `src/shim.rs`.

- [ ] **Step 2: Fix up every cross-reference (fix-up procedure)**

Known call sites for `shim::` (excluding the ones Task 2 already resolved): `src/jvm_thread.rs` had `shim::define_classes` — that's now inside `jvm/inbox.rs` (post Task 4) as `crate::jvm::entry::define_classes`. Grep for any remaining `shim::` anywhere in the crate and fix to `jvm::entry::`. Run `cargo build`, fix what it reports, repeat to green.

- [ ] **Step 3: Verify and commit**

Run: `cargo test --lib`, `cargo clippy --all-targets && cargo fmt --check`, and the full ERT suite:

```bash
export EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars"
EMACS_JNI_TEST_FEATURES=test-support ./test/run-ert-tests.sh
```

Expected: 16 test files run, 83 tests, 0 unexpected — the verified pre-reorg baseline (confirmed during this plan's authoring: `EMACS_JNI_CLOJURE_JARS_DIR=.../target/clojure-jars EMACS_JNI_TEST_FEATURES=test-support ./test/run-ert-tests.sh`, no arguments, ran all 16 files). This is the first ERT run of the plan (end of the whole `jvm/` side) — cheaper to batch here than after every single Rust-only task.

```bash
git add -A
git commit -m "refactor(rust): Move shim.rs into jvm/entry.rs, complete the jvm/ side"
```

---

### Task 7: Build `lisp/convert.rs`

**Files:**
- Create: `src/lisp/convert.rs`
- Modify: `src/lib.rs` (loses `lisp_value_to_arg`)
- Modify: `src/lisp_requests.rs` (loses `arg_to_value`, `bridge_value_to_value`, `value_to_outcome`)

**Interfaces:**
- Consumes: `crate::protocol::{BridgeValue, LispInput, LispOutcome}` (Task 1), `crate::jvm::convert::{JvmObject, JvmHandle}` (Task 2, for boxing an object into a Lisp `Value`).
- Produces: `crate::lisp::convert::{to_bridge, from_bridge, from_input, to_outcome}` — `lisp/entry.rs` (Task 10) and `lisp/inbox.rs` (Task 9) call these.

- [ ] **Step 1: Create the directory and file, move + rename**

`mkdir -p src/lisp`. Move into `src/lisp/convert.rs`:
- `lisp_value_to_arg` from `src/lib.rs:285-368` → rename `to_bridge`.
- `bridge_value_to_value` from `src/lisp_requests.rs:208-245` → rename `from_bridge`.
- `arg_to_value` from `src/lisp_requests.rs:183-207` → rename `from_input`.
- `value_to_outcome` from `src/lisp_requests.rs:337-412` → rename `to_outcome`.

- [ ] **Step 2: Fix up every cross-reference (fix-up procedure)**

Known call sites (from the earlier grep of `lisp_value_to_arg`/`bridge_value_to_value`/`arg_to_value`/`value_to_outcome`): `src/lib.rs` (several — `lisp_value_to_arg(...)` → `lisp::convert::to_bridge(...)`, `lisp_requests::bridge_value_to_value` → `lisp::convert::from_bridge`, `lisp_requests::arg_to_value` → `lisp::convert::from_input`, `lisp_requests::value_to_outcome` → `lisp::convert::to_outcome`), `src/lisp_requests.rs`'s own remaining code (e.g. `run_request_value`/`compute_reply` likely call these siblings internally — same renames), `src/jvm/entry.rs` — this task runs after Task 6, so the old `shim.rs` code that referenced these is already there under its new name. Run `cargo build`, fix what it reports, repeat to green. Create `src/lisp.rs` with `mod convert;`.

- [ ] **Step 3: Verify and commit**

Run: `cargo test --lib` and `cargo clippy --all-targets && cargo fmt --check`.

```bash
git add -A
git commit -m "refactor(rust): Extract lisp/convert.rs from lib.rs and lisp_requests.rs"
```

---

### Task 8: Build `lisp/runtime.rs`

**Files:**
- Create: `src/lisp/runtime.rs`
- Delete: `src/reentry.rs`

**Interfaces:**
- Produces: `crate::lisp::runtime::{EnvScope, enter, in_scope, with_current_env}` (unchanged names) — `jvm/entry.rs` (Task 6) and `jvm/invoke.rs`/`lisp/entry.rs` (direct-call path) consume these.

- [ ] **Step 1: Move**

Move `src/reentry.rs` in full into `src/lisp/runtime.rs`. No renames. Delete `src/reentry.rs`.

- [ ] **Step 2: Fix up every cross-reference (fix-up procedure)**

Known call sites for `reentry::` (from the earlier grep): `src/lib.rs` (`reentry::enter`), `src/jvm/entry.rs` (post-Task-6 home of the old `shim.rs`'s `reentry::in_scope`/`with_current_env` calls). Replace `reentry::` with `lisp::runtime::` at each. Run `cargo build`, fix what it reports, repeat to green. Add `mod runtime;` to `src/lisp.rs`.

- [ ] **Step 3: Verify and commit**

Run: `cargo test --lib` and `cargo clippy --all-targets && cargo fmt --check`.

```bash
git add -A
git commit -m "refactor(rust): Move reentry.rs into lisp/runtime.rs"
```

---

### Task 9: Build `lisp/inbox.rs`

Merges the rest of `lisp_requests.rs` with `channel.rs`, fusing enqueue+poke into one `submit` per the spec's Open Question 3 (verified behavior-preserving: `channel::poke` already no-ops silently when uninitialized, and every current call site already pairs the two 1:1).

**Files:**
- Create: `src/lisp/inbox.rs`
- Delete: `src/lisp_requests.rs`, `src/channel.rs`

**Interfaces:**
- Consumes: `crate::lisp::convert::{from_bridge, from_input, to_outcome}` (Task 7), `crate::protocol::{Status, LispOutcome}` (Task 1).
- Produces: `crate::lisp::inbox::{Inbound, submit, init, service_inbound}` — `submit` replaces the separate `enqueue`+`channel::poke` pair; `init` replaces `channel::init`; `lisp/entry.rs` (Task 10) calls both.

- [ ] **Step 1: Move and fuse**

Move into `src/lisp/inbox.rs`: `src/lisp_requests.rs`'s remaining content — `Inbound` enum (`:66-134`), `QUEUE`/`enqueue`/`drain` (`:136-158`), `box_value`/`borrow` (`:159-182`), `reply`/`error_to_reply`/`free_reply_ptr`/`fallback_signal` (`:246-336`), `service_inbound`/`handle_message`/`run_request_value`/`compute_reply` (`:413-end`), plus the `#[cfg(test)] mod tests`. Also move `src/channel.rs`'s `WRITER`, `init`, `poke` in full.

Fuse: change `pub fn enqueue(req: Inbound)` to a private `fn enqueue` (or inline it), and define:

```rust
/// Adds REQ to the queue and wakes the main thread to drain it — the two
/// were always meant to be called together (a message enqueued without a
/// poke sits undrained until the next unrelated wakeup); fusing them removes
/// the possibility of forgetting the second call.
pub fn submit(req: Inbound) {
    enqueue(req);
    poke();
}
```

Every call site that used to do `enqueue(x); channel::poke();` (or `crate::lisp_requests::enqueue(...); crate::channel::poke();`) now does one `submit(x)` call instead.

- [ ] **Step 2: Fix up every cross-reference (fix-up procedure)**

Known call sites (from the earlier greps of `lisp_requests::`/`channel::`): `src/jvm/inbox.rs` (Task 4's destination for the old `jvm_thread.rs` lifecycle code) and `src/jvm/invoke.rs` (Task 5's destination for the old `jvm_thread.rs` reflection code) — every `crate::lisp_requests::enqueue(X); ... channel::poke();` pair there becomes one `crate::lisp::inbox::submit(X)`; `src/jvm/entry.rs` (Task 6's destination for `shim.rs`) — same fusion for its `enqueue`+`poke` pairs (e.g. old `shim.rs:253-254`, `:299-300`, `:454-455`, `:649`, `:673-674`); `src/lib.rs` — `lisp_requests::service_inbound` → `lisp::inbox::service_inbound`, `channel::init` → `lisp::inbox::init`. Run `cargo build`, fix what it reports, repeat to green. Add `mod inbox;` to `src/lisp.rs`.

- [ ] **Step 3: Verify and commit**

Run: `cargo test --lib` and `cargo clippy --all-targets && cargo fmt --check`.

```bash
git add -A
git commit -m "refactor(rust): Merge lisp_requests.rs and channel.rs into lisp/inbox.rs, fuse enqueue+poke into submit"
```

---

### Task 10: Build `lisp/entry.rs`, shrink `lib.rs`

Final task: moves every remaining `#[defun]` out of `lib.rs`, leaving it as a pure manifest.

**Files:**
- Create: `src/lisp/entry.rs`
- Modify: `src/lib.rs` (shrinks to `mod` declarations + whatever Step 2 below determines about `#[emacs::module]` placement)

**Interfaces:**
- Consumes: `crate::lisp::convert::to_bridge` (Task 7), `crate::lisp::inbox::{init, service_inbound}` (Task 9), `crate::lisp::runtime::enter` (Task 8), `crate::jvm::inbox::{submit, next_request_id, start_or_resume, clear_sender, Cmd}` (Task 4), `crate::jvm::invoke::{call_direct, DirectError}` (Task 5), `crate::jvm::runtime::{vm, loader, set_main_thread, set_main_attached}` (Task 3), `crate::jvm::convert::JvmHandle` (Task 2).

- [ ] **Step 1: Confirm the `#[emacs::module]` placement is safe to move**

Verified already (this plan's authoring session read `emacs-macros-0.17.0/src/module.rs` and `emacs-0.21.0/src/init.rs`): the macro's default feature name comes from `::emacs::init::lisp_pkg(module_path!())`, which only uses the *crate-name* segment of the module path (`mod_path.split("::").nth(0)`) — so it's unaffected by which submodule the `#[emacs::module]`-annotated `init` function lives in. The registration itself (`::emacs::__module_init!`) expands to a plain `#[no_mangle] pub unsafe extern "C" fn emacs_module_init`, which works from any module. **`init` can move to `lisp/entry.rs` along with every other `#[defun]`.** No open question remains.

- [ ] **Step 2: Move**

Move every remaining item in `src/lib.rs` into `src/lisp/entry.rs`: `init` (`:19-27`), `jvm_ping` (`:29-40`), `jvm_init_channel` (`:42-46`), `jvm_start` (`:47-57`), `jvm_pause` (`:58-68`), `jvm_stop` (`:69-92`), `jvm_load_jar` (`:93-110`), `jvm_load_jar_direct` (`:111-124`), `jvm_call` (`:125-146`), `parse_call_target` (`:147-170`), `jvm_call_direct` (`:171-221`), `jvm_service_inbound` (`:222-224`), `jvm_invoke_reply` (`:226-266`), `lisp_list_to_args` (`:268-284`), `jvm_test_panic` (`:369-382`), `string_list` (`:383-390`). (`lisp_value_to_arg` already moved in Task 7 — confirm it's gone.) No renames.

- [ ] **Step 3: Shrink `lib.rs` to a pure manifest**

```rust
mod protocol;
mod lisp;
mod jvm;
```

- [ ] **Step 4: Fix up every remaining cross-reference (fix-up procedure)**

Every reference inside the moved code that used a bare/short-form import (`worker::`, `runtime::`, `commands::`, `lisp_requests::`, `channel::`, `reentry::`, `jvm_thread::`) now needs its final `jvm::`/`lisp::` qualified form, per every earlier task's rename table. This is the last file to migrate, so `cargo build` should now report the *complete* remaining set — run it, fix everything reported, repeat to green. This is the strongest single checkpoint in the whole plan: a clean build here means the entire crate's cross-references are consistent.

- [ ] **Step 5: Full verification**

```bash
cargo test --lib
cargo clippy --all-targets
cargo fmt --check
export EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars"
EMACS_JNI_TEST_FEATURES=test-support ./test/run-ert-tests.sh
```

Expected: `cargo test --lib` 22 passed (same as the pre-reorg baseline, confirmed during Task 1); ERT 16 files, 83 tests, 0 unexpected (identical to Task 6's run).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(rust): Move lib.rs's defuns into lisp/entry.rs, shrink lib.rs to a manifest"
```

---

### Task 11: Final sweep

Catches anything the per-task fix-ups missed and confirms the tree matches the spec exactly.

**Files:** none new — this is a verification-only task.

- [ ] **Step 1: Confirm the final tree matches the spec**

```bash
find src -name '*.rs' | sort
```

Expected output exactly:

```
src/jvm.rs
src/jvm/convert.rs
src/jvm/entry.rs
src/jvm/inbox.rs
src/jvm/invoke.rs
src/jvm/runtime.rs
src/lib.rs
src/lisp.rs
src/lisp/convert.rs
src/lisp/entry.rs
src/lisp/inbox.rs
src/lisp/runtime.rs
src/protocol.rs
```

- [ ] **Step 2: Grep for any leftover old-module reference**

```bash
grep -rn -E "\b(commands|marshal|handles|lisp_requests|jvm_thread|shim|worker|pending|channel|runtime|reentry|status)::" src/
```

Expected: no matches (every reference now uses `protocol::`, `lisp::{convert,inbox,runtime,entry}::`, or `jvm::{convert,inbox,invoke,runtime,entry}::`). If anything matches, fix it and re-run `cargo build`.

- [ ] **Step 3: Full test suite, one last time**

```bash
cargo test --lib
cargo clippy --all-targets
cargo fmt --check
export EMACS_JNI_CLOJURE_JARS_DIR="$(pwd)/target/clojure-jars"
EMACS_JNI_TEST_FEATURES=test-support ./test/run-ert-tests.sh
```

- [ ] **Step 4: Commit if Step 2 required any fixes**

```bash
git add -A
git commit -m "refactor(rust): Clean up remaining old-module references"
```

If Step 2 found nothing, there's nothing to commit — the plan is complete as of Task 10's commit.
