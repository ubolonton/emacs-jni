# Rust

- `cargo clippy` and `cargo fmt` before committing.
- Prefer `thiserror` for library errors, `anyhow` for application-level error handling.
- If you are not familiar with a library, prefer reading its source code over guessing. You can use `cargo metadata` to find the source directory.

## Style and safety

- PREFER integration tests over unit tests
- NEVER perform builds with the release profile, unless asked or reproducing performance issues
- PREFER running specific tests over running the entire test suite
- AVOID using `panic!`, `unreachable!`, `.unwrap()`, unsafe code, and clippy rule ignores
- PREFER patterns like `if let` to handle fallibility
- ALWAYS write `SAFETY:` comments to describe to explain the soundness when writing `unsafe` code
- PREFER `#[expect()]` over `[allow()]` if clippy must be disabled
- PREFER let chains (`if let` combined with `&&`) over nested `if let` statements
- NEVER update all dependencies in the lockfile and ALWAYS use `cargo update --precise` to make
  lockfile changes
- NEVER assume clippy warnings are pre-existing, it is very rare that `main` has warnings
- ALWAYS read and copy the style of similar tests when adding new cases
- PREFER top-level imports over local imports or fully qualified names
- AVOID shortening variable names, e.g., use `version` instead of `ver`, and `requires_python`
  instead of `rp`

## Code comments

- Comment only when code can't express the intent — names and types first
- Inline comments explain *why*, not *what*; full sentences; 1–3 lines.
- Explain nuances, summarize big gnarly chunks of code, state assumptions about incoming data and behavior of external systems.
- Include ref links with concise summary: Jira ticket, Slack convo...
- Doc comments cover *when/why to use*, not just what.
- Use TODO/FIX/XXX prefix for things that won't be worked on soon. Be explicit and include the reason.
        - `TODO`: General improvements.
        - `FIX`: An bug or undesired behavior to be fixed.
        - `XXX`: An undesired behavior that is complex, for which no obvious solution is available.
