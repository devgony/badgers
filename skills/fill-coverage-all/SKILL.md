---
name: fill-coverage-all
description: Raise repository-wide test coverage with Badgers, regardless of what changed in the current PR. Use early in a project or when asked to "fill coverage for the whole repo", "raise overall coverage", or "add tests for untested code". Iterates module by module with `badgers cov --all --path` until each target area is covered. For PR-scoped gaps, use the fill-coverage skill instead.
---

# Fill Coverage (Repo-Wide)

Backstop untested code across the entire repository by working through
uncovered executable lines module by module. Unlike `fill-coverage`, this
targets ALL code, not just lines changed in the current pull request.

## Prerequisites

- `badgers` CLI installed **and recent enough to provide `badgers cov`** —
  verify and install/upgrade with the steps in "Verify `badgers cov` first" below
- Rust projects: `cargo llvm-cov` installed (`cargo install cargo-llvm-cov`)
- Python projects: tests already run under `coverage run` before converting

## Verify `badgers cov` first

`badgers cov` is the only local loop condition for this skill. Before anything
else, confirm the installed CLI actually provides it:

```bash
badgers cov --help >/dev/null 2>&1 && echo "cov ok" || echo "cov missing"
```

If it prints `cov missing`, the CLI is either absent
(`command not found: badgers`) or too old
(`error: unrecognized subcommand 'cov'`). Install or upgrade it — do NOT hunt
for a different subcommand, there is no substitute for `cov`.

```bash
# Prebuilt binary, no Rust toolchain (installs to ~/.local/bin/badgers — put it on PATH)
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/devgony/badgers/releases/latest/download/badgers-installer.sh | sh

# Or from crates.io (requires Rust)
cargo install --force badge-rs

# Or from a badgers checkout when a freshly installed release still lacks `cov`
make install   # cargo install --locked --force --path crates/badgers-cli
```

Re-run the check and confirm it prints `cov ok` before continuing.

## Workflow

### 1. Survey the whole repository

```bash
badgers cov --all --no-fail
```

Prints every uncovered executable line in the repo:

```text
Coverage: 120 uncovered executable lines
Local: main @ 427d215
Total coverage: 66.67% (no baseline)
src/parser.rs:42,47-60 [uncovered]
src/store.rs:91-140 [uncovered]
```

Use `--no-fail` during the survey so a large gap does not read as an error.

### 2. Pick ONE module and agree on scope

Do not attack the whole list at once. Group the output by directory, then
choose the next target by value: core business logic first, then parsing
and IO boundaries, generated or glue code last. If the user has not
specified a target, propose an ordering and confirm before writing tests.

### 3. Iterate on the chosen module only

```bash
badgers cov --all --path src/parser
```

- `--path` scopes the report, the totals, and the exit code to that prefix,
  so this is the loop condition for the module.
- Write the smallest meaningful tests that execute the listed lines,
  following the project's existing test conventions.
- Re-run until the module reports `Coverage: no uncovered executable lines`
  (exit 0), then commit and move to the next module.

### 4. Track overall progress

Between modules, re-run the survey (`badgers cov --all --no-fail`) and
report the shrinking total to the user.

## Rules

- One module per iteration; commit per module so progress is reviewable.
- Meaningful tests only: assert observable behavior, not implementation
  details. Never write a test whose only purpose is executing a line.
- 100% is not the goal. If a line is unreasonable to cover (defensive
  branch, platform guard, generated code), list it in the summary as
  intentionally uncovered instead of forcing a test.
- Do not refactor production code to chase coverage; report untestable
  designs to the user instead.
- If the pull request gate (`fail-on-uncovered`) is what actually failed,
  switch to the `fill-coverage` skill — it is faster and correctly scoped.
