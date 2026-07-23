# Badgers

Badgers is a coverage checker for Rust, Python, and Flutter projects. It keeps an eye on pull requests, compares each push against the base branch, and reports whether line coverage improved, dropped, or left changed lines uncovered.

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

Coding agents can read the latest stored coverage diff directly in the
terminal. The pull request number is optional when the current branch has an
open pull request discoverable by GitHub CLI:

```bash
badgers diff 547
# or, from the pull request branch
badgers diff
```

The output is a compact, deterministic list of uncovered changed executable
lines. It also includes total and changed-line coverage summaries; complete
human-oriented reports remain available through Markdown and `badgers view`.

While filling coverage gaps, agents iterate locally without waiting for CI:

```bash
badgers cov
```

The `cov` command runs coverage in the working tree (`cargo llvm-cov` for
Rust, `flutter test --coverage` for Flutter, `coverage.py` for Python, or a
prebuilt file via `--lcov-file`), diffs
the working tree against the pull request base, and prints the same compact
format as `badgers diff`. It exits with code 1 while uncovered changed
executable lines remain, so the loop is: read the stored diff once, add
tests, re-run `badgers cov` until it exits 0, then push. Pass `--no-fail`
for report-only mode and `--baseline <snapshot.json>` to include the total
coverage delta. The bundled `skills/fill-coverage` skill packages this
workflow for coding agents.

Early-stage projects can target the whole repository instead of a pull
request diff:

```bash
badgers cov --all --path src/parser
```

With `--all`, every uncovered executable line is reported regardless of
what changed, and repeatable `--path` prefixes scope the list, the totals,
and the exit code to one module at a time. The `skills/fill-coverage-all`
skill drives this module-by-module loop for coding agents.

Install the skills once per repository and commit the result so every
teammate and coding agent picks them up on the next pull:

```bash
npx skills add devgony/badgers@fill-coverage --copy
npx skills add devgony/badgers@fill-coverage-all --copy
git add .agents .claude skills-lock.json && git commit -m "chore: add badgers coverage skills"
```

The skills CLI detects installed agents automatically; extra flags are not
required for interactive use. `--copy` writes real files instead of
symlinks so checkouts on Windows work without developer mode. Update later
with `npx skills update -p`.

Both commands infer the source repository from the checkout's local `origin`.
The `view` command follows `prs/547/latest.json`, caches the referenced HTML
bundle, and opens its self-contained `index.html`. Same-repository storage is
cloned directly through the existing Git remote, so repository detection does
not require a GitHub API call. Use `--repo`, `--storage-repo`,
`--storage-branch`, and `--storage-prefix` when the report storage differs
from the defaults. Cross-repository storage uses the authenticated GitHub CLI.
Pass `--no-open` to download the bundle and print its exact local path without
opening a browser.

CI workflows do not need to install the binary themselves. Versioned
`devgony/badgers` Action releases download the matching prebuilt binary and
verify its checksum. Development refs such as `main`, commit SHAs, unsupported
platforms, and releases without binary assets fall back to building from
source.

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
- Flutter coverage via `flutter test --coverage` (set `language: flutter` in
  the action)
- Shared parsing through LCOV first, JSON later

## Storage

Badgers supports GCS and GitHub repository storage. Both backends store the
same snapshot and pointer layout, and both can supply a pull request baseline
without re-running the test suite at the merge-base. A typical layout looks
like this:

```text
gs://coverage-bucket/badgers/repos/{owner}/{repo}/
  commits/{sha}/coverage.json.zst
  commits/{sha}/lcov.info.zst
  refs/main/latest.json
  prs/{number}/latest.json
```

Badgers does not upload generated snapshots or reports as GitHub Actions
artifacts. Use GCS or GitHub repository storage for durable coverage history.

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

`cli-version` defaults to `auto`: `@v1` selects the newest stable `v1.x.y`
release containing binaries for the current runner, while an exact Action ref
such as `@v1.2.3` selects only that release. Set an exact `cli-version` to use a
released CLI from `main` or a commit SHA, set it to `latest` to track the newest
stable prebuilt CLI independently of the Action ref, or set it to `source` to
force a local release build.

`markdown-summary` is opt-in. When enabled, Badgers adds a navigable coverage
report to the GitHub Actions job summary. With GitHub repository storage
enabled, the complete Markdown report is also stored as `README.md` alongside
the snapshot and HTML bundle.

`fail-on-uncovered` defaults to `false`. When enabled, the action fails after
all reports, comments, and snapshots are published if the pull request still
contains uncovered changed executable lines. Combined with branch protection,
this blocks merging until the coverage gap is closed; agents then backfill
tests locally with `badgers cov` until the gate passes.

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
a dedicated Git branch. On pull requests, Badgers reads the exact merge-base
snapshot from this branch when available, then falls back to the latest base
branch pointer. It only re-runs the coverage command at the merge-base when
neither snapshot is stored. `github-storage-branch` defaults to
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

## Development

Install the CLI from the current checkout with:

```bash
make install
```

Before the first release, add a crates.io API token as the repository Actions
secret `CARGO_REGISTRY_TOKEN`. The token must be authorized to publish all five
`badge-rs*` packages. GitHub CLI must also be authenticated locally.

Create a release from a clean, pushed `main` branch with:

```bash
make release
```

The prompt accepts `major`, `minor`, or `patch` (or `1`, `2`, or `3`). Automation
can bypass the prompt with an exact named bump:

```bash
make release BUMP=patch
```

The release target rejects any mismatch between the workspace version, internal
dependency pins, CLI manifest, and lockfile. It updates all of them, runs the
Python tests, formatting check, workspace tests, and Clippy, then creates a
version commit and annotated tag. The commit and immutable version tag are
pushed atomically before the GitHub Release is published. Releases are stable
semantic versions only; prerelease bumps are not supported. The moving `v1`,
`v2`, and so on tag continues to advance after each release in that major line.

Publishing the GitHub Release starts crate publication and four-platform binary
packaging from the same `release.published` event. Crates are published in
dependency order: `badge-rs-core`, `badge-rs-github`, `badge-rs-storage`,
`badge-rs-lcov`, then `badge-rs`. The workflow waits for crates.io propagation,
skips package versions that already exist when rerun, and replaces existing
binary assets. crates.io publication and GitHub asset uploads are separate
services and cannot be one atomic transaction; rerun the release workflow for
the existing tag if either job fails.

## Status

Badgers implements PR line coverage, coverage delta, diff coverage, GitHub PR
comments and Check annotations, plus GCS- and GitHub-repository-backed history.
The project is being dogfooded on this repository while the public action API is
stabilized.
