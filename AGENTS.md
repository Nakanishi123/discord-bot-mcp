- ALWAYS ensure that new tests use the same style as existing tests for all parts of the test
- ALWAYS check whether the behavior of a new test is already covered by an existing test
- PREFER running specific tests over running the entire test suite
- AVOID using `panic!`, `unreachable!`, `.unwrap()`, unsafe code, and clippy rule ignores
- PREFER patterns like `if let` to handle fallibility
- PREFER exhaustive `match` expressions without wildcard (`_`) arms when handling enums,
  so new enum variants require explicit handling
- ALWAYS write `SAFETY` comments following our usual style when writing `unsafe` code
- PREFER `#[expect()]` over `#[allow()]` if clippy must be disabled
- NEVER update all dependencies in the lockfile and ALWAYS use `cargo update --precise` to make
  lockfile changes
- NEVER assume clippy warnings or test failures are pre-existing; investigate them before proceeding
- AVOID shortening variable names, e.g., use `version` instead of `ver`, and `requires_python`
  instead of `rp`
- DO NOT leak our conversation, prompt, or iteration history into code comments, pull request
  descriptions, or other maintainer-facing prose. Write for readers who have not seen our
  conversation.
- PREFER comments that explain the current behavior and rationale. Avoid past-facing wording like
  "preserve the existing behavior"; explain the actual backwards-compatibility constraint instead.
