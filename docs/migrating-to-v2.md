# Migrating to Badgers v2

Badgers v2 replaces the language-specific coverage collectors with one
tool-agnostic LCOV contract. Your test command owns coverage generation;
Badgers reads the resulting LCOV file and connects it to snapshots, baselines,
pull request reports, and coverage gates.

## GitHub Action

`coverage-command` must now both run the tests and write the file named by
`lcov-file`. The default path is `coverage/lcov.info`.

### Python

Before:

```yaml
- uses: devgony/badgers@v1
  with:
    coverage-command: python -m coverage run -m pytest
```

After:

```yaml
- uses: devgony/badgers@v2
  with:
    coverage-command: >-
      python -m coverage run -m pytest &&
      mkdir -p coverage &&
      python -m coverage lcov -o coverage/lcov.info
    lcov-file: coverage/lcov.info
```

### Rust

```yaml
- uses: devgony/badgers@v2
  with:
    coverage-command: >-
      cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info
    lcov-file: coverage/lcov.info
```

### Flutter

```yaml
- uses: devgony/badgers@v2
  with:
    coverage-command: flutter test --coverage
    lcov-file: coverage/lcov.info
```

Complete workflow examples are available in [`examples/workflows/`](../examples/workflows).

## CLI

The language-specific commands have been replaced:

```text
badgers collect python   -> badgers collect lcov
badgers collect flutter -> badgers collect lcov
```

The generic command reads `coverage/lcov.info` by default:

```bash
badgers collect lcov --repo-root . -o coverage-snapshot.json
```

Use `--lcov-file <PATH>` when your coverage tool writes elsewhere.

## Pinning

The moving major tag is now `devgony/badgers@v2`. Exact-version pins can use
`devgony/badgers@v2.0.0`. Do not combine the v2 action with a v1 CLI through
`cli-version`; the v1 CLI does not provide `badgers collect lcov`.
