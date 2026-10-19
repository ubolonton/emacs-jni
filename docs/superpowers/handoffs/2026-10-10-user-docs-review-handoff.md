# Handoff: user-docs review findings and prevention

Scope: `README.md`, `docs/{index,tutorials,how-to-guides,reference,explanation}/` (not
`docs/superpowers/`). Judged against [Diátaxis](https://diataxis.fr/) and ASD-STE100
(STE-flavored mode). Docs were rewritten on 2026-10-10 after this review; see
`git log --grep='docs:'` for the fixes. Related: the code-comment cleanup handoff,
`2026-10-10-comment-slop-prevention-handoff.md` (same root causes, same prevention shape).

## Findings (pre-fix state)

| Rank | Doc | Main defect |
|---|---|---|
| 1 | tutorials/02 | Step 4 failed (verified): `jvm-clj-eval-string` needs `emacs-jni.clj` loaded; the tutorial loaded jars only. |
| 2 | explanation/architecture | Core premise false since the direct path became default ("one worker thread owns the JVM, Lisp never touches the JVM directly"). Stale `-sync`, `lisp_requests::Inbound`. |
| 3 | explanation/interop-design | All converter names stale; "token" (old handle table); false claim "a Java field can hold only a primitive". Worst STE metrics (avg 26.5 words/sentence, 55% > 25 words). |
| 4 | reference/lisp-api | Broken markdown table (jvm-clj.el); false row "symbol naming a function → callable" (symbols cross as `LispSymbol`); no `LispSymbol` rows; stale `jvm-handle`. |
| 5 | explanation/limitations | Stale `src/jvm_thread.rs`, `-sync`; Clojure→Lisp wait section ignores direct path; defensive tone. |
| 6 | README | Project-status/roadmap prose, Phase-numbered superpowers links. |
| 7 | how-tos | False: `-async` takes a timeout; symbol → callable; overload example (long vs String is deterministic; real ambiguity is numeric: `String.valueOf(42)` → `"42.0"`). Non-task headings restating limitations. |
| 8 | reference/configuration | Claimed `mise.toml`/`mise.lock` untracked (they are tracked). |
| 9–10 | tutorials/01, index | Good. Index is the model. |

Cross-cutting causes:

1. **Doc rot after a design change.** When bare names became `-direct`, reference/how-tos were
   patched; explanations were not. No step in the workflow checks docs against code.
2. **Duplication.** The same limitation lived in 3–4 places, which then drifted.
3. **Audience mixing.** Rust internals (function paths, crate versions, test flakiness) in user
   docs. These rot fastest, because refactors rename them.
4. **Agent prose habits.** Long em-dash sentences, history ("found empirically"), defensive
   tone ("not an oversight"), synonym rotation (`-sync`/`-await`; wrapped object/user-ptr/
   handle/token).

Code issue found (not fixed): `jvm::convert::marshal_error` reuses `jni::errors::Error::ParseFailed`,
so every application error reaches Lisp prefixed "Parse failed for input:". Fix with a crate
error type, or strip the prefix where `jvm-error` is signalled.

## Prevention measures (proposed, not done)

1. **Executable docs.** Add an ERT test (or `mise` task) that extracts each tutorial's elisp
   blocks with `;; =>` results and runs them in `emacs --batch`. Tutorial 2's breakage would
   have failed CI. Start with tutorials; extend to how-to snippets that use fixtures.
2. **Stale-identifier check.** A script that collects backticked identifiers in `docs/` and
   `README.md` that look like code (`foo::bar`, `src/*.rs`, `jvm-*`/`jvm--*` symbols) and fails
   if `rg` finds no definition. Run it as a `mise` task and in the PostToolUse hook from the
   comment-slop handoff.
3. **Single source per fact.** Rule: limitations live only in `explanation/limitations-and-
   tradeoffs.md`; how-tos and reference link to it. Rust internals live only in the contributor
   page (`docs/explanation/implementation.md`) or code comments.
4. **Definition of done for design changes.** Add to the plan template under
   `docs/superpowers/`: "List every user doc that describes the changed behavior; update or
   mark each." Make the final whole-branch review check it.
5. **Prose rule.** Extend the rule drafted in the comment-slop handoff to docs: STE-flavored,
   ≤25-word sentences, no history, no defensive asides, one term per concept (glossary in
   `docs/index.md` or reference).

## Open questions for the user

- Gate on executable docs in `run-ert-tests.sh`, or a separate `mise` task?
- Should the stale-identifier check block (hook exit 2) or warn?

## Suggested skills

- `update-config`: for any hook.
- `asd-ste100`: for the prose rule text.
- `superpowers:writing-plans`: if prevention work gets a plan.
