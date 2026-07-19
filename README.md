# Badgers

Badgers is a coverage checker for Rust and Python projects. It keeps an eye on pull requests, compares each push against the base branch, and reports whether line coverage improved, dropped, or left changed lines uncovered.

![Badgers logo](./images/logo-badgers.png)

## Install

```bash
cargo install badge-rs
```

The installed binary is `badgers`. One-time GCS provisioning (bucket, workload
identity federation, service account, IAM bindings) then takes a single input:

```bash
badgers setup gcs --project YOUR_GCP_PROJECT_ID
```

CI workflows do not need the binary: the `devgony/badgers` GitHub Action builds
and runs it on the runner.

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
  contents: write # `read` is sufficient when repository storage/Pages are disabled
  id-token: write
  pull-requests: write
  checks: write

steps:
  - uses: actions/checkout@v4
    with:
      fetch-depth: 0
      ref: ${{ github.event.pull_request.head.sha || github.sha }}

  - uses: google-github-actions/auth@v3
    with:
      project_id: my-gcp-project
      workload_identity_provider: projects/123456789/locations/global/workloadIdentityPools/github/providers/github
      service_account: coverage-writer@my-gcp-project.iam.gserviceaccount.com

  - uses: devgony/badgers@v1
    with:
      gcs-bucket: company-coverage
      gcs-prefix: badgers/repos/jubilee-works/timetree-planner-server
      github-storage-repo: jubilee-works/coverage-reports
      github-storage-token: ${{ secrets.BADGERS_STORAGE_TOKEN }}
      markdown-summary: true
```

`markdown-summary` is opt-in. When enabled, Badgers adds a navigable coverage
report to the GitHub Actions job summary and uploads the same report as the
`coverage-markdown` artifact. The existing HTML artifact is still produced.

`check-annotations` defaults to `true`. With `checks: write`, Badgers creates a
`Badgers diff coverage` check and places warnings for uncovered changed
executable lines directly in the pull request's **Files changed** view. Fork
pull requests remain read-only and skip publishing. Set the input to `false`
when `checks: write` is unavailable to suppress permission warnings. The source
checkout must use the pull request head SHA as shown above; annotations are
skipped when the analyzed checkout does not match that SHA.

## GitHub Repository Storage

Set `github-storage-repo` to keep browsable reports and compressed snapshots in
a dedicated Git branch. `github-storage-branch` defaults to
`badgers-reports`; `github-storage-prefix` defaults to `badgers`.

```text
badgers/repos/{owner}/{repo}/
  commits/{sha}/README.md
  commits/{sha}/coverage.json.zst
  commits/{sha}/comparison.json.zst
  refs/{branch}/latest.json
  refs/{branch}/README.md
  prs/{number}/latest.json
  prs/{number}/README.md
```

The default `github-storage-token` works when the storage repository is the
repository running the workflow and the job grants `contents: write`. For a
different or private repository, pass a GitHub App installation token or a
fine-grained PAT with Contents write access. Repository writes are skipped for
pull requests from forks.

## Status

Badgers is currently a project design. The first milestone is PR line coverage, coverage delta, diff coverage, GitHub PR reporting, and GCS-backed history.
