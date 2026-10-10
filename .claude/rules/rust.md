# Rust

- `cargo clippy` and `cargo fmt` before committing.
- Prefer `thiserror` for library errors, `anyhow` for application-level error handling.
- If you are not familiar with a library, prefer reading its source code over guessing. You can use `cargo metadata` to find the source directory.
- Hard-wrap comments and docstrings at 100 characters.

## Style

- PREFER integration tests over unit tests
- NEVER perform builds with the release profile, unless asked or reproducing performance issues
- PREFER running specific tests over running the entire test suite
- AVOID using `panic!`, `unreachable!`, `.unwrap()`, unsafe code, and clippy rule ignores
- PREFER patterns like `if let` to handle fallibility
- PREFER `#[expect()]` over `[allow()]` if clippy must be disabled
- PREFER let chains (`if let` combined with `&&`) over nested `if let` statements
- NEVER update all dependencies in the lockfile and ALWAYS use `cargo update --precise` to make
  lockfile changes
- NEVER assume clippy warnings are pre-existing, it is very rare that `main` has warnings
- ALWAYS read and copy the style of similar tests when adding new cases
- PREFER top-level imports over local imports or fully qualified names
- AVOID shortening variable names, e.g., use `version` instead of `ver`, and `requires_python`
  instead of `rp`

## Docstrings
- Doc (`///`) comments cover *when/why to use*, not just what; use `# Safety`, `# Implementation`, `# Examples` sections; cross-link types with `` [`Type`] `` syntax.

## Safety
- Every `unsafe fn` must have a `# Safety` doc section explaining the contracts the caller must uphold.
- Every `unsafe {}` block must have a `// SAFETY:` comment explaining why the usage is sound.
  - If soundness relies on Emacs behavior rather than Rust guarantees, state it explicitly as an assumption.
