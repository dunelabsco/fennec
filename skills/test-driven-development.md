---
name: test-driven-development
description: Write a failing test before writing the code. Use for new features, bug reproductions, and any change whose correctness is not obvious from reading it.
always: false
---

# test-driven-development

The loop is simple and non-negotiable:

1. **Write a test that fails** — the smallest test that captures one aspect of what should be true.
2. **Run it** — confirm it fails for the reason you expect. A test that fails for the wrong reason is worse than no test.
3. **Write the minimum code to make it pass** — only that. Resist extra scope.
4. **Run the full test suite** — new test green, nothing else red.
5. **Refactor** — with all tests passing, clean up. The tests guard the refactor.
6. **Commit** — failing test + passing code + refactor is one logical step; commit them together or in clear consecutive commits.

## When TDD is worth the friction

- New public function, endpoint, or boundary behaviour.
- Reproducing a bug before fixing it (the test IS the reproduction).
- Pure logic: parsing, calculations, state machines.
- Any change where "this obviously works" is wishful thinking.

## When TDD gets in the way

- Exploratory spikes to learn a library — prototype first, test after the shape is clear.
- UI polish, one-line typo fixes, trivial renames.
- External-API integration before you know the response shape.

## Test sizing

- A test should fail for exactly one reason. Multiple assertions are OK if they describe one behaviour; multiple behaviours belong in multiple tests.
- Name after the behaviour, not the function: `returns_empty_list_when_user_has_no_messages`, not `test_get_messages_3`.
- Structure each test as Arrange / Act / Assert (or Given / When / Then). Blank lines separate the sections; explicit comments only when the structure isn't obvious.

## Running tests

| Language | Invocation |
|---|---|
| Rust | `cargo test` (narrow with `-p <crate>` and a test name) |
| Python | `pytest` (`pytest path/to/test.py::test_name`) |
| Node | `npm test` or `vitest` / `jest` depending on the project |
| Go | `go test ./...` (narrow with `-run <pattern>`) |

Narrow during the inner loop. Full suite before commit.

## Anti-patterns

- Writing the test after the code and calling it TDD. That is test-after-development — useful, but the loop is broken.
- Tests that mock every collaborator and assert the mocks were called. They test the mock, not the code.
- One "god test" per feature that fails for ten different reasons.
- Skipping the red step. A test you never saw fail has not been verified.
