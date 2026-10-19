# Handoff: prevent AI-slop comments and docstrings

## Goal

Add guardrails so agents stop writing the comment style that was cleaned out of `src/` on
2026-10-10 (see `git log --grep='docs(rust)'` for the cleanup commits; diffs show before/after).

## Diagnosis (why the old comments were bad)

1. History instead of current state: "was", "originally", "before Task 6", "now gone".
2. Defending the code to a reviewer: "not a bug", "by intent", "load-bearing proof".
3. One sentence per paragraph: nested em-dashes and parentheses, main clause buried.
4. Intensifier tics: real, exactly, actually, just, its own, simply, rather than.
5. Cross-reference webs: every comment names 3-5 other functions; references rot
   (`shim.rs`, `worker.rs` no longer existed).
6. Wrong altitude: design essays in fn docstrings; restating what code shows.
7. Stale forward-looking notes: "Task 4 will likely extend this enum".

Probable root cause: plan-executing subagents get "Task N: ..." prompts; the task framing
leaks into code. The existing rule in `~/.claude/rules/coding.md` ("no task numbers", "focus on
current state") was too abstract to win against that context.

## Agreed approach

Rule + hook. A skill was rejected: it only loads on trigger, and comments get written in passing
during every edit.

1. **Rule**: extend `~/.claude/rules/coding.md` "Code comments and docstrings" with concrete
   anti-patterns (the user's global rules; not in this repo). Draft:
   ```markdown
   - State the current invariant. No history: "was", "originally", "no longer", "before X".
   - Don't defend the code ("not a bug", "by intent", "load-bearing").
   - One idea per sentence. No nested em-dashes or parentheses.
   - Avoid intensifiers: real, exactly, actually, just, its own, simply.
   - Docstring = contract + one-line why. Put design reasoning in docs/ and link to it.
   - Reference other code only when the reader must go there.
   ```
2. **Hook**: `PostToolUse` on Edit/Write. Grep *added* comment lines for
   `\b(Task|Item|Phase|Step|Finding) ?\d`, `§`, and backticked `*.rs` paths that don't exist.
   On match, return feedback to the model. Decide scope: user-global settings vs this project's
   `.claude/settings.json`. Prefer a helper script over inline shell (user's coding rules).
3. **Source fix**: plan/handoff templates under `docs/superpowers/` (and the superpowers
   `writing-plans` / `subagent-driven-development` prompts, if customizable) should say "never
   put task IDs or plan references in code".

## Open questions for the user

- Hook scope: global or per-project?
- Hook action: block (exit 2) or warn?

## Suggested skills

- `update-config`: for the hook in settings.json.
- `superpowers:writing-skills`: only if revisiting the skill option.
- `asd-ste100`: wording of the rule text.
