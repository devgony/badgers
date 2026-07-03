# Badgers

Badgers is a coverage checker for Rust and Python projects. It keeps an eye on pull requests, compares each push against the base branch, and reports whether line coverage improved, dropped, or left changed lines uncovered.

![Badgers logo](./images/logo-badgers.png)

## What It Does

- Measures line coverage on every pull request update.
- Compares PR coverage against the latest successful base branch snapshot.
- Reports total coverage change and diff coverage for changed lines.
- Stores coverage history outside GitHub Actions artifacts.
- Uses Google Cloud Storage as the default backend.
- Runs as a GitHub Action, with the core implementation written in Rust.

## Language Support

Badgers uses proven ecosystem tools and normalizes their output into one coverage model:

- Rust coverage via `cargo llvm-cov`
- Python coverage via `coverage.py`
- Shared parsing through LCOV first, JSON later

## Storage

Badgers is GCS-first. A typical layout looks like this:

```text
gs://coverage-bucket/badgers/repos/{owner}/{repo}/
  commits/{sha}/coverage.json.zst
  commits/{sha}/lcov.info.zst
  refs/main/latest.json
  prs/{number}/latest.json
```

GitHub Actions artifacts can be supported later for small projects or demos, but they are not the recommended default for long-term coverage history.

## GitHub Actions Sketch

```yaml
permissions:
  contents: read
  id-token: write
  pull-requests: write
  checks: write

steps:
  - uses: actions/checkout@v4

  - uses: google-github-actions/auth@v3
    with:
      project_id: my-gcp-project
      workload_identity_provider: projects/123456789/locations/global/workloadIdentityPools/github/providers/github
      service_account: coverage-writer@my-gcp-project.iam.gserviceaccount.com

  - uses: jubilee-works/badgers-action@v1
    with:
      storage: gcs
      gcs-bucket: company-coverage
      gcs-prefix: badgers/repos/jubilee-works/timetree-planner-server
```

## Status

Badgers is currently a project design. The first milestone is PR line coverage, coverage delta, diff coverage, GitHub PR reporting, and GCS-backed history.
