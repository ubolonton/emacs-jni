# `src/` module reorganization — design

Status: design approved by user, not yet implemented. Pure reorganization — file
moves and renames only. No behavior change; no test expectations change.

## Motivation

The Lisp↔JVM conversion and call-dispatch code (`lib.rs`, `marshal.rs`,
`lisp_requests.rs`, `jvm_thread.rs`, `shim.rs`, `handles.rs`, `commands.rs`,
`pending.rs`, `worker.rs`, `channel.rs`, `runtime.rs`, `reentry.rs`, `status.rs`)
grew file-by-file across several past specs. Individually reasonable choices left
a structure that doesn't communicate itself:

- Function names (`lisp_value_to_arg`, `lisp_arg_to_jvalue`, `jvm_value_to_lisp_arg`,
  `bridge_value_to_value`) don't self-explain direction without reading the body.
- `lisp_value_to_arg` (Lisp→bridge conversion) lives in `lib.rs`, while its
  return-direction counterpart `bridge_value_to_value` lives in `lisp_requests.rs`
  — the two halves of "how Lisp converts" are in different files.
- `lisp_requests.rs` conflates the inbound queue (transport/threading) with data
  conversion.
- `handles.rs` (`JvmObject`/`JvmHandle`) is conceptually part of JVM-side value
  representation but sits in its own file for no structural reason.
- Files that are tightly coupled by execution context (`lib.rs`+`lisp_requests.rs`
  both run on the Emacs main thread; `shim.rs`+`jvm_thread.rs`+`pending.rs` all run
  attached to the JVM worker thread) don't signal that relationship anywhere in the
  tree.

## Goals

- A file tree where the file a symbol lives in tells you its role without opening it.
- Perfect structural symmetry between the Lisp-facing and JVM-facing code: the same
  four roles (entry / convert / inbox / runtime) exist on both sides, named the same
  way.
- No behavior change. No test file changes. A green `cargo test` + full ERT suite is
  the only acceptance criterion.

## Non-goals

- Renaming `BridgeValue`/`CallTarget`/`LispOutcome`/`LispInput` or changing their
  variants — that vocabulary was already settled by the 2026-09-18
  marshalling-consolidation work and reads fine.
- Any change to the async/direct/`-await` call semantics. (`-await` is confirmed
  Elisp-only — a blocking wrapper over the async path in `elisp/jvm-clj.el` — so it
  has no Rust-side presence to relocate.)
- Fixing the enqueue-then-poke hazard noted below (see Open questions) as part of
  this move — flagged, not fixed, to keep this reorg behavior-preserving.

## Design principles

Four roles, present on both sides:

- **entry** — the surface the *other* runtime calls into (a `#[defun]` for Lisp
  calling in; a `Lisp.*` JNI native for the JVM calling in).
- **convert** — this side's value ↔ `BridgeValue`/`LispInput`/`LispOutcome`
  translation. Only this file on each side imports that side's foreign-runtime
  types (`emacs::Value` for `lisp/`, `jni::objects::*` for `jvm/`).
- **inbox** — the async, fire-and-forget path: submit a message, it runs on this
  side's own thread/context, a reply may go back out. Owns the message envelope
  type (`Inbound` for `lisp/`, `Cmd` for `jvm/`) and everything that services it
  — this is the "receives requests from the other side" facility, named
  consistently on both sides instead of `inbox.rs`/`worker.rs`. The "submit"
  half (today split across `channel.rs`/`worker.rs`) folds in as each inbox's own
  submit function, so a caller never enqueues without also waking the receiver.
- **runtime** — the synchronous, direct-access handle the *other* side's
  direct-call path reaches into to bypass the inbox entirely: `jvm/runtime.rs`
  (JavaVM + classloader + status) is what `lisp/`'s `jvm-call-direct` path uses;
  `lisp/runtime.rs` (the env-stack, today's `reentry.rs`) is what `jvm/`'s
  reentrant callback path uses. Each is a facility one side provides *for* the
  other's direct path — that's the test for whether something belongs here.

One file sits outside both sides: **`protocol.rs`** holds the payload/value types
that cross the Lisp/JVM boundary and are constructed by one side, consumed by the
other — `BridgeValue`, `CallTarget`, `LispOutcome`, `LispInput`, `Status`. The
message *envelopes* (`Cmd`, `Inbound`) are NOT here — an envelope belongs to the
side whose inbox receives it, since it's that inbox's own mailbox format, not
shared vocabulary. `protocol.rs` stays free of `jni`-crate types with one
preserved exception: `BridgeValue::Object`/`CallTarget::Object` hold a
`crate::jvm::convert::JvmObject`, the same cross-reference `commands.rs` has
today (via `handles::JvmObject`) — not a new coupling, just relocated.

Root-file style: no `mod.rs`. Each side's root is a plain sibling file
(`src/lisp.rs` next to `src/lisp/`, `src/jvm.rs` next to `src/jvm/`), and it holds
**only** `mod` declarations — no logic, not even the entry-point code. `entry` is
a leaf file like every other role, so the root file has zero special cases and
both roots are trivially small and identically shaped.

## Proposed tree

```text
src/
├── lib.rs              # mod declarations + module init glue only
├── protocol.rs          # BridgeValue, CallTarget, LispOutcome, LispInput, Status
├── lisp.rs              # mod entry; mod convert; mod inbox; mod runtime;
├── lisp/
│   ├── entry.rs          # the #[defun]s (today's lib.rs body)
│   ├── convert.rs        # Lisp Value <-> BridgeValue/LispInput/LispOutcome
│   ├── inbox.rs          # Inbound + submit() (enqueue+poke) + drain loop
│   └── runtime.rs        # env-stack for jvm/'s reentrant direct-path callbacks
├── jvm.rs                # mod entry; mod convert; mod inbox; mod invoke; mod runtime;
└── jvm/
    ├── entry.rs           # the Lisp.* JNI natives (today's shim.rs)
    ├── convert.rs         # BridgeValue/LispInput <-> JValue/JObject, + JvmObject/JvmHandle
    ├── inbox.rs           # Cmd + submit() (send_command) + worker thread loop +
    │                      # lifecycle (create_jvm/pause/stop/resume/load_jar) +
    │                      # pending (reply-matching table, folded in)
    ├── invoke.rs           # call machinery shared by async and direct:
    │                       # resolve_and_invoke, build_invoke_args, invoke_method,
    │                       # invoke_constructor, call_direct, classloader
    │                       # resolution, non-local-exit building
    └── runtime.rs           # JavaVM + classloader handle + JVM status
                             # (StatusCell), for lisp/'s direct-call path
```

## Mapping (old → new)

| Old | New | Notes |
|---|---|---|
| `commands.rs::BridgeValue`, `CallTarget`, `LispOutcome` | `protocol.rs` | unchanged |
| `lisp_requests.rs::LispInput` | `protocol.rs` | unchanged; only ever appears inside `Inbound`, but it's a boundary payload like `BridgeValue` |
| `status.rs::Status`, `as_tag` | `protocol.rs` | the enum is a payload (`Inbound::Status.status`) |
| `status.rs::StatusCell`, `STATUS` | `jvm/runtime.rs` | mutable JVM-side state, same role as the VM/classloader handle |
| `commands.rs::Cmd` | `jvm/inbox.rs` | envelope type — belongs to the receiving side |
| `lisp_requests.rs::Inbound` | `lisp/inbox.rs` | envelope type — belongs to the receiving side |
| `lib.rs` (every `#[defun]`: `init`, `jvm_ping`, `jvm_init_channel`, `jvm_start`, `jvm_pause`, `jvm_stop`, `jvm_load_jar[_direct]`, `jvm_call[_direct]`, `jvm_service_inbound`, `jvm_invoke_reply`, `jvm_test_panic`) + private helpers `parse_call_target`, `string_list`, `lisp_list_to_args` | `lisp/entry.rs` | unchanged bodies; `jvm_init_channel` now calls `lisp::inbox::init`, `jvm_call_direct` now calls `jvm::runtime::vm/loader` + `jvm::invoke::call_direct` |
| `lib.rs::lisp_value_to_arg` | `lisp/convert.rs::to_bridge` | Lisp `Value` → `BridgeValue` |
| `lisp_requests.rs::bridge_value_to_value` | `lisp/convert.rs::from_bridge` | `BridgeValue` → Lisp `Value` |
| `lisp_requests.rs::arg_to_value` | `lisp/convert.rs::from_input` | `LispInput` → Lisp `Value` (Lisp-access sub-protocol) |
| `lisp_requests.rs::value_to_outcome` | `lisp/convert.rs::to_outcome` | Lisp `Value` → `LispOutcome` |
| `lisp_requests.rs::{enqueue, drain, box_value, borrow, service_inbound, handle_message, run_request_value, compute_reply, reply, error_to_reply, free_reply_ptr, fallback_signal}` | `lisp/inbox.rs` | the drain loop and its `Env`-side reply-building helpers |
| `channel.rs::{init, poke}` | `lisp/inbox.rs` | folded into `submit()`, so enqueue and wakeup can no longer be called separately |
| `reentry.rs` (whole file) | `lisp/runtime.rs` | unchanged: `EnvScope`, `enter`, `in_scope`, `with_current_env` |
| `handles.rs` (whole file) | `jvm/convert.rs` | `JvmObject`, `JvmHandle` + its `Drop` (now calls `jvm::inbox::submit`) |
| `marshal.rs::lisp_arg_to_jvalue` | `jvm/convert.rs::from_bridge` | `BridgeValue` → JNI `JValue` |
| `marshal.rs::{jvm_value_to_lisp_arg, object_to_lisp_arg}` | `jvm/convert.rs::to_bridge` | JNI `JObject`/`JValue` → `BridgeValue` |
| `marshal.rs::{register_release_on_gc, big_integer_from_str, class_name, marshal_error, box_if_primitive, box_via_value_of}` | `jvm/convert.rs` | private/`pub(crate)` helpers, unchanged |
| `shim.rs::object_to_lisp_request_arg` | `jvm/convert.rs::to_input` | JNI `JObject` → `LispInput` |
| `shim.rs::jobject_array_to_lisp_request_args` | `jvm/convert.rs` | loops `to_input`; stays next to it |
| `shim.rs` (everything else: `define_classes`, all `register_*_native`, `lisp_function_call`, `lisp_intern`, `lisp_type_of`, `lisp_is_not_nil`, `lisp_eq`, `lisp_apply_async`, `lisp_object_prin1_to_string`, `service_lisp_request_{sync,direct}`, `block_on_lisp_reply`, `rethrow_lisp_reply_exception`, `release_notifier_run`, `lisp_ref_release_notifier_run`, `get_handle_field`, `get_ptr_field`, `unbox_boolean`, `define_if_absent`) | `jvm/entry.rs` | unchanged bodies |
| `worker.rs::{next_request_id, start_or_resume, send_command, clear_sender}` | `jvm/inbox.rs` | `send_command` becomes `submit` |
| `pending.rs` (whole file) | `jvm/inbox.rs` | the reply-matching table `Cmd::InvokeReply`/`LispReply` resolve against |
| `jvm_thread.rs::{run, is_first_attempt, create_jvm, panic_message, handle_pause, handle_stop, handle_resume, handle_load_jar, load_jar_direct, create_shared_classloader, add_jar_url, handle_invoke_reply, handle_lisp_reply}` | `jvm/inbox.rs` | the worker thread's event loop + lifecycle |
| `jvm_thread.rs::{build_lisp_reply_object, complete_future_with_reply, build_non_local_exit, throw_non_local_exit, handle_call, resolve_and_invoke, call_direct, resolve_class_via_loader, describe_caught_java_exception, is_overload_arg_mismatch, extract_non_local_exit, invoke_method, invoke_constructor, build_invoke_args}` | `jvm/invoke.rs` | shared by async (`handle_call`, in `jvm/inbox.rs`) and direct (`call_direct`) |
| `runtime.rs` (whole file) | `jvm/runtime.rs` | unchanged: `set_vm`, `vm`, `get_or_create_loader`, `loader`, `set_main_thread`, `on_main_thread`, `main_attached`, `set_main_attached`, `clear` |

## Decisions (from discussion)

- Scope: the whole `src/` tree, not just the files named in the original complaint.
- Subdirectories are allowed; Rust 2021 (this crate's edition) supports the
  `foo.rs` + `foo/` sibling style used throughout, no `mod.rs` anywhere.
- Pivot function naming: `to_bridge`/`from_bridge` (short names, module path
  carries the side), not `encode_lisp_to_bridge`-style verbose names.
- Top-level split is by which native runtime the code speaks to (`lisp`/`jvm`),
  not by execution context (`main_thread`/`worker_thread`) — same boundary,
  clearer name.
- `entry` is a leaf file (`lisp/entry.rs`, `jvm/entry.rs`), not inlined into the
  root (`lisp.rs`/`jvm.rs`) — keeps the root's job exceptionless (manifest only).

## Open questions for the planner / user

1. **`#[emacs::module]` placement.** The `init` function's module-registration
   attribute may need to stay physically in `lib.rs` per the `emacs` crate's
   macro requirements (this project's user is the crate's author and can likely
   answer directly, or it can be checked empirically during implementation). If
   so, `lib.rs` keeps just that one function (or re-exports `lisp::entry::init`
   for the attribute to wrap); everything else in the mapping is unaffected.
2. **Root files' visibility.** `lisp.rs`/`jvm.rs` are expected to need nothing
   beyond `mod` declarations — cross-file callers use fully-qualified paths
   (`crate::jvm::convert::to_bridge`, `crate::lisp::runtime::enter`, etc.). Confirm
   no call site actually wants a shorter re-exported path; add `pub(crate) use`
   only if the plan finds one that reads noticeably worse fully qualified.
3. **Enqueue/poke fusion in `lisp/inbox.rs::submit`.** Folding `channel::poke`
   into `submit` (so a caller can no longer enqueue without waking the drain
   loop) is a genuine safety improvement — today's code relies on every call site
   remembering to call both, an already-flagged hazard from the
   2026-09-18 marshalling-consolidation spec. It's included in the mapping above
   as the natural shape of the merge, not as a deliberate behavior change, but
   flagging it explicitly: confirm you want this, since it's the one place this
   reorg's file merge also tightens a correctness invariant rather than being a
   pure rename.

## Sequencing and risk

Pure move-and-rename; the main risk is a long chain of broken imports mid-move,
not logic errors. Recommended order, compiling after each step:

1. Introduce `protocol.rs` (move `BridgeValue`/`CallTarget`/`LispOutcome` from
   `commands.rs`, `LispInput` from `lisp_requests.rs`, `Status`/`as_tag` from
   `status.rs`); delete `commands.rs`; fix imports crate-wide.
2. Build `jvm/` bottom-up: `jvm/convert.rs` (merge `marshal.rs` + `handles.rs` +
   `to_input`), `jvm/runtime.rs` (move `runtime.rs` + `StatusCell`/`STATUS`),
   `jvm/inbox.rs` (merge `worker.rs` + `pending.rs` + `jvm_thread.rs`'s loop
   half), `jvm/invoke.rs` (`jvm_thread.rs`'s reflection half), `jvm/entry.rs`
   (`shim.rs` minus `to_input`); add the `jvm.rs` manifest.
3. Build `lisp/` bottom-up the same way: `lisp/convert.rs`, `lisp/runtime.rs`
   (`reentry.rs`), `lisp/inbox.rs` (remaining `lisp_requests.rs` + `channel.rs`),
   `lisp/entry.rs` (`lib.rs`'s defuns); add the `lisp.rs` manifest.
4. Shrink `lib.rs` to `mod protocol; mod lisp; mod jvm;` (+ resolved Open
   Question 1).
5. `cargo clippy && cargo fmt`, then full `cargo test` + ERT suite (per
   `test/run-ert-tests.sh`, with `EMACS_JNI_TEST_FEATURES=test-support` so
   `jvm-crash`/`jvm-handle` also run) — zero test file changes expected; any
   red test means an import/visibility slip, not a design problem.

Each numbered step above is a reasonable commit boundary.

## Testing strategy

No new tests — this is a pure structural move. Gate: `cargo test` (all unit
tests, unchanged) and the full ERT suite green, byte-for-byte identical test
files. If any test needs a source change beyond an import path, that's a signal
the move altered behavior and needs to be re-examined, not patched around.
