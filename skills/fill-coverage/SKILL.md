---
name: fill-coverage
description: Fill in missing tests until the Badgers coverage gate passes. Use when a pull request's "Badgers diff coverage" check failed, when asked to "fill coverage", "fix coverage diff", or "add tests for uncovered lines". Iterates locally with `badgers cov` until no uncovered changed executable lines remain, then pushes and confirms CI.
---

# Fill Coverage

Close the coverage gap on the current pull request by writing tests for
exactly the changed lines that CI reported as uncovered.

## Prerequisites

- `badgers` CLI installed (`~/.local/bin/badgers` or `cargo install badge-rs`)
- GitHub CLI (`gh`) authenticated, current branch has an open pull request
- Rust projects: `cargo llvm-cov` installed (`cargo install cargo-llvm-cov`)
- Python projects: tests already run under `coverage run` before converting

## Workflow

### 1. Bootstrap the target list (free, no local coverage run)

```bash
badgers diff
```

This reads the comparison stored by CI. Each line like

```text
src/parser.rs:42,47-48 [changed-uncovered]
```

is a changed executable line with no test coverage. These are your targets.
If `badgers diff` fails because no comparison is stored yet, skip to step 3
and let `badgers cov` compute everything locally.

### 2. Write tests for exactly those lines

- Open each listed file and read the listed lines plus surrounding context.
- Write the smallest tests that execute those lines. Follow the project's
  existing test conventions (inline `#[cfg(test)]` modules, `tests/`
  directory, pytest layout, etc.).
- Do not refactor production code to make it "more testable" unless a line
  is genuinely unreachable; in that case report it instead of forcing a test.

### 3. Verify locally (the loop condition)

```bash
badgers cov
```

- Runs coverage locally (auto-detects `cargo llvm-cov` / `coverage.py`),
  diffs the working tree against the PR base, and prints the same format
  as `badgers diff`.
- Exit code `1` and a fresh `[changed-uncovered]` list → return to step 2
  using this NEW list (not the stale one from step 1).
- Exit code `0` (`Coverage diff: no uncovered changed executable lines`) →
  proceed to step 4.

Never re-run `badgers diff` inside this loop: it reflects the last CI run,
not your local edits. `badgers cov` is the only loop condition.

### 4. Push and confirm CI

```bash
git push
gh pr checks --watch
```

CI recomputes the comparison and the coverage gate should now pass. If it
unexpectedly fails while `badgers cov` was clean, compare the CI coverage
command with your local run (flags, excluded files, feature sets) and align
them before iterating again.

## Rules

- Only the `[changed-uncovered]` lines matter. Do not chase total-coverage
  percentages or add tests for files you did not change.
- Keep tests meaningful: assert observable behavior, not implementation
  details, even when targeting specific lines.
- If a listed line cannot be reasonably covered (defensive branch, platform
  guard), say so explicitly in the final summary instead of writing a fake
  assertion.
