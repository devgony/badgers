# Badgers

Badgers is a coverage checker for Rust and Python projects. It keeps an eye on pull requests, compares each push against the base branch, and reports whether line coverage improved, dropped, or left changed lines uncovered.

![Badgers logo](./images/logo-badgers.png)

## Install

Install a prebuilt binary without Cargo:

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/devgony/badgers/releases/latest/download/badgers-installer.sh | sh
```

The installer detects macOS/Linux and ARM64/x86-64, verifies the release
checksum, and writes the binary to `~/.local/bin/badgers`. Rust developers can
instead build from crates.io:

```bash
cargo install badge-rs
```

The installed binary is `badgers`. One-time GCS provisioning (bucket, workload
identity federation, service account, IAM bindings) then takes a single input:

```bash
badgers setup gcs --project YOUR_GCP_PROJECT_ID
```

When GitHub repository storage is enabled, download and open the latest HTML
report for a pull request with:

```bash
badgers view 547
```

The command infers the source repository from the checkout's local `origin`
remote, follows `prs/547/latest.json`, caches the referenced HTML bundle, and
opens its self-contained `index.html`. Same-repository storage is cloned
directly through the existing Git remote, so repository detection does not
require a GitHub API call. Use `--repo`, `--storage-repo`,
`--storage-branch`, and `--storage-prefix` when the report storage differs
from the defaults. Cross-repository storage uses the authenticated GitHub CLI.
Pass `--no-open` to download the bundle and print its exact local path without
opening a browser.

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

The compact pull request comment links directly to the durable detailed
Markdown report when repository storage is enabled, the pull request's
**Files changed** annotations, and the optional hosted HTML report. The
`durable-report-url` action output exposes the stable Markdown report URL for
same-repository pull requests; it is empty when storage is disabled or the pull
request comes from a fork. The existing `report-url` output continues to expose
the optional Pages-hosted HTML report.

Badgers keeps one marker-based coverage comment per pull request and refreshes
it with the latest result. Before updating, it verifies that the analyzed head
SHA is still the pull request's current head. Configure the calling workflow
with PR-scoped concurrency so delayed runs cannot overwrite newer results:

```yaml
concurrency:
  group: badgers-${{ github.event.pull_request.number || github.ref }}
  cancel-in-progress: true
```

Without workflow-level serialization, comment updates remain best-effort
because GitHub's issue-comment API does not provide an atomic compare-and-swap.

Markdown reports keep commit-pinned blob links as the primary file navigation.
Changed files also include a best-effort **PR diff** link pinned to the analyzed
head commit so the corresponding Files changed section and Check annotations
are easy to inspect. GitHub does not document its per-file `#diff-…` anchors,
so these auxiliary links may change with GitHub's web UI.

## GitHub Repository Storage

Set `github-storage-repo` to keep browsable reports and compressed snapshots in
a dedicated Git branch. `github-storage-branch` defaults to
`badgers-coverage`; `github-storage-prefix` defaults to `badgers`.

```text
badgers/repos/{owner}/{repo}/
  commits/{sha}/README.md
  commits/{sha}/coverage.json.zst
  commits/{sha}/comparison.json.zst
  commits/{sha}/html/          ← HTML report bundle (index.html, assets/, …)
  refs/{branch}/latest.json
  refs/{branch}/README.md
  prs/{number}/latest.json
  prs/{number}/README.md
```

Pass `--html-report <DIR>` (or set it in the action) to store the generated
`coverage-report/` directory alongside the snapshot. Every file in the tree is
written to `commits/{sha}/html/{relative_path}`, and `html_prefix` in the
pointer JSON records the bundle root so tooling can locate it later.

**HTML is not renderable via GitHub's blob or raw URLs.** Use `badgers view
<PR>` to download and open the self-contained bundle. You can also check out
the storage branch and open the referenced `index.html`, run a local web
server, or deploy the bundle to a static host separately.

The storage branch always contains exactly **one parentless (orphan) commit**
per push. History never grows: each run replaces the branch entirely with a
single fresh commit. For this to work, the storage branch must **not** be
branch-protected. The push uses `--force-with-lease` (matching the SHA cloned
at the start of the run) so that two concurrent jobs fail-and-retry rather than
silently clobbering each other.

**Retention.** Set `github-storage-retention: latest` (the default) to
automatically prune `commits/{sha}` directories that are no longer referenced
by any `refs/*/latest.json` or `prs/*/latest.json` pointer after each push.
Use `all` to skip pruning and keep all historical commit bundles. Retention
operates only on the temporary clone and cannot reach the GCS bucket; GCS
baselines are always safe.

The default `github-storage-token` works when the storage repository is the
repository running the workflow and the job grants `contents: write`. For a
different or private repository, pass a GitHub App installation token or a
fine-grained PAT with Contents write access. Repository writes are skipped for
pull requests from forks.

## Status

Badgers implements PR line coverage, coverage delta, diff coverage, GitHub PR
comments and Check annotations, plus GCS- and GitHub-repository-backed history.
The project is being dogfooded on this repository while the public action API is
stabilized.
