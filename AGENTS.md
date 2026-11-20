## Style
* Prioritise simplicity and consistency alongside code being idiomatic.

## Test Rules
* Tests should add enough value to offset the bloat they add to the codebase.
* Skip tests for framework behaviour (clap validation, serde deserialization, etc.). The framework already tests itself.
* Skip tests for trivial arithmetic or straightforward transformations that are self-evident from reading the code.
* Add tests when the logic is non-obvious, contains edge cases, or encodes business rules that could regress silently.
* Prefer tests that verify behaviour over tests that verify implementation details.

## Lint Rules
* New code should not introduce additional clippy warnings (latest stable).
