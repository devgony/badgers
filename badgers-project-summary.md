# Badgers 프로젝트 논의 요약

작성일: 2026-07-03 (storage 관련 내용은 현재 구현인 GitHub repository branch storage 기준으로 갱신)

## 한 줄 요약

Badgers는 Rust와 Python 프로젝트의 line coverage를 PR마다 계산하고, 기존 baseline 대비 증감과 diff coverage를 비교해서 GitHub PR에 리포트하는 자체 coverage 서비스/액션이다.

## 목표

- PR에 신규 push가 생길 때마다 line coverage를 계산한다.
- base branch의 기존 coverage와 PR head coverage를 비교한다.
- 전체 line coverage 증감과 변경 라인 기준 diff coverage를 보여준다.
- Coveralls처럼 편하게 쓰되, 사내 비용과 저장소 제약을 피하기 위해 coverage history를 사용자가 지정한 GitHub repository의 전용 branch에 저장한다.
- 우선 사내에서 쓰고, 이후 오픈소스화할 수 있는 구조로 만든다.

## 프로젝트 이름

최종 후보로 **Badgers**를 선택했다.

이름을 고른 이유:

- `badger`에는 끈질기게 캐묻고 물고 늘어진다는 의미가 있어 coverage regression을 꼼꼼히 잡는 도구와 잘 맞는다.
- `badgers`는 `badge + rs`처럼도 읽혀 Rust 감성이 있다.
- Rust와 Python을 모두 지원해도 특정 언어에 너무 묶이지 않는다.
- repo, CLI, crate 이름으로 자연스럽다.

권장 네이밍:

```text
Product: Badgers
Repo: badgers
CLI: badgers
Crate: badgers
GitHub Action: badgers-action
Storage prefix: badgers/
```

태그라인 후보:

```text
Badgers is a coverage checker that keeps badgering your pull requests until every changed line is accounted for.
```

## 지원 언어

Badgers 자체는 Rust로 구현한다.

다만 coverage 측정기를 직접 새로 만들기보다는 각 언어 생태계의 검증된 도구를 실행하고, 결과를 공통 모델로 정규화한다.

Rust:

- `cargo llvm-cov` 사용
- LCOV 또는 JSON 출력 사용
- line coverage를 1차 지원

Python:

- `coverage.py` 사용 (LCOV 출력은 coverage.py 6.3 이상 필요)
- `coverage run -m pytest`
- `coverage combine`
- `coverage lcov` 또는 `coverage json`

공통 처리:

```text
Rust coverage report
Python coverage report
        |
        v
LCOV/JSON parser
        |
        v
CoverageSnapshot
        |
        v
compare / store / report
```

초기 버전은 line coverage만 공통 기능으로 잡는다. Branch coverage는 Rust/LLVM과 Python coverage.py의 모델 차이가 있으므로 후속 확장으로 분리한다.

## 저장소 결정

GitHub Actions artifact는 기본 저장소로 쓰지 않는다.

이유:

- 현재 확인한 `jubilee-works` organization의 GitHub plan은 `team`이다.
- GitHub Team의 Actions artifact shared storage allowance는 문서 기준 2GB다.
- 이 2GB는 Actions artifacts와 GitHub Packages가 공유한다.
- 사내 여러 팀이 공유하는 quota를 coverage history로 채우는 것은 피하는 것이 좋다.

GitHub Actions cache는 repo당 10GB지만, 재생성 가능한 cache 용도에 가깝고 장기 coverage history의 원장으로는 부적합하다.

## 기본 storage: GitHub repository branch

durable storage는 GitHub repository의 **전용 branch** 하나다. Action input으로 지정한다:

- `github-storage-repo`: 저장소 repository (`owner/repo`). 비어 있으면 storage를 쓰지 않는다.
- `github-storage-branch`: 전용 branch (기본 `badgers-coverage`)
- `github-storage-prefix`: branch 안의 경로 prefix (기본 `badgers`)
- `github-storage-token`: contents-write token (기본 `github.token`)
- `github-storage-retention`: `latest`(기본, pointer가 참조하지 않는 `commits/{sha}` 정리) 또는 `all`

구조 (storage branch 안):

```text
badgers/
  repos/
    jubilee-works/
      timetree-planner-server/
        commits/{sha}/coverage.json.zst
        commits/{sha}/comparison.json.zst
        commits/{sha}/README.md
        commits/{sha}/html/
        refs/{encoded_branch}/latest.json
        refs/{encoded_branch}/README.md
        prs/{pr_number}/latest.json
        prs/{pr_number}/README.md
```

주의: branch 이름에는 `/`가 들어갈 수 있으므로 (`release/1.2` 등) 경로에 넣을 때 percent encoding한다. default branch가 `main`이 아닐 수 있으므로 `refs/main`을 하드코딩하지 않는다.

실행 중 생성되는 head/baseline snapshot, comparison, Markdown/HTML report는 runner의 임시 파일일 뿐이며 job이 끝나면 사라진다. GitHub Actions artifact로 업로드하지 않는다. 선택적으로 `pages: true`이면 HTML report를 GitHub Pages branch(기본 `gh-pages`)에도 배포하지만, baseline은 storage branch에서만 읽는다.

보관 정책:

- storage branch는 push마다 parentless commit 하나로 교체되므로 git history가 쌓이지 않는다. 따라서 branch protection을 걸지 않는다.
- `github-storage-retention: latest`이면 어떤 `latest.json`도 참조하지 않는 `commits/{sha}`를 push마다 정리한다. 다른 사본은 없으므로, 정리된 merge-base snapshot은 복구할 수 없고 base branch 최신 snapshot(approximate baseline)으로 fallback한다.
- 모든 commit snapshot을 남기려면 `all`을 쓴다.

## Storage 인증 방식

별도 cloud credential은 필요 없다. storage branch push에는 `github-storage-token`을 쓴다.

- storage repository가 workflow를 실행하는 repository와 같으면 기본 `github.token`과 job의 `contents: write`로 충분하다.
- 다른 repository나 private repository라면 Contents write 권한을 가진 GitHub App installation token 또는 fine-grained PAT를 넘긴다.
- fork PR에서는 storage write를 건너뛴다. baseline 읽기는 read-only token 또는 anonymous clone(public storage repository)으로 시도한다.

GitHub Actions 예시:

```yaml
permissions:
  contents: write
  pull-requests: write
  checks: write

steps:
  - uses: actions/checkout@v4
    with:
      fetch-depth: 0  # merge-base 계산과 git diff base...head에 필요

  - uses: devgony/badgers@v2
    with:
      coverage-command: >-
        cargo llvm-cov --workspace --lcov --output-path coverage/lcov.info
      github-storage-repo: jubilee-works/coverage-reports
      github-storage-token: ${{ secrets.BADGERS_STORAGE_TOKEN }}
```

## 기본 동작 흐름

PR 이벤트:

1. `pull_request`의 `opened`, `synchronize`, `reopened`에서 실행한다.
2. Rust coverage를 생성한다.
3. Python coverage를 생성한다.
4. 두 report를 공통 snapshot으로 병합한다.
5. storage branch를 얕게 clone해 baseline snapshot을 읽는다. 우선순위: merge-base commit의 snapshot (`commits/{merge_base_sha}/`) → 없으면 base branch의 최신 성공 snapshot → 둘 다 없으면 merge-base worktree에서 coverage command를 다시 실행 (`base-rebuild: auto`).
6. 전체 coverage 증감을 계산한다.
7. `git diff base...head` 기준 변경 라인만 뽑아 diff coverage를 계산한다.
8. GitHub Check Run과 PR comment를 작성한다.
9. 현재 commit의 snapshot, comparison, report를 storage branch에 commit/push한다 (fork PR은 생략).

Base branch push:

1. main 또는 default branch push에서 coverage를 계산한다.
2. commit별 snapshot을 `commits/{sha}/`에 저장하고, `refs/{encoded_branch}/latest.json` 포인터를 갱신한다.
3. latest.json 갱신 시 commit timestamp를 비교해 더 오래된 commit이 최신 포인터를 덮어쓰지 않게 한다. 동시 push 간 race는 storage branch push의 `--force-with-lease`(clone 시점 SHA 기준)로 감지하고, 다시 clone해 재시도한다.
4. 이후 PR 비교의 baseline으로 사용한다.

## 비교 기준

전체 coverage 증감:

```text
PR head snapshot - baseline snapshot
```

Baseline 선택 우선순위:

1. **merge-base commit의 snapshot** (`git merge-base base head` 기준). PR이 분기된 이후 base branch에 들어간 변경이 PR의 증감으로 잘못 귀속되는 것을 막는다.
2. merge-base snapshot이 없으면 base branch의 최신 성공 snapshot (`refs/{branch}/latest.json`)으로 fallback하고, 리포트에 "approximate baseline"임을 명시한다.

Diff coverage:

```text
git diff base...head의 추가/수정 라인 중 covered line 비율
```

Base snapshot이 없을 때:

- storage에 baseline이 없으면 PR workflow에서 merge-base를 checkout해 coverage를 한 번 더 계산한다 (`base-rebuild: auto`, 기본값).
- `base-rebuild: never`이면 baseline 없이 head-only report를 만든다. diff coverage는 계속 계산한다.
- storage를 쓰면 base branch workflow가 먼저 실행되어 있을수록 rebuild가 줄어든다.

## 내부 데이터 모델 초안

```rust
struct CoverageSnapshot {
    schema_version: u32,
    repo: String,
    commit_sha: String,
    branch: Option<String>,
    pr_number: Option<u64>,
    generated_at: String,
    files: Vec<FileCoverage>,
}

struct FileCoverage {
    path: String,
    language: Language,
    // executable_lines/covered_lines는 line_hits에서 유도되는 캐시 값이다.
    // canonical source는 line_hits이며, 역직렬화 시 재검증한다.
    executable_lines: u32,
    covered_lines: u32,
    line_hits: Vec<LineHit>,
}

struct LineHit {
    line: u32,
    hits: u32,
}

enum Language {
    Rust,
    Python,
    Unknown,
}
```

## CLI 초안

```text
badgers run
badgers collect rust
badgers collect python
badgers merge
badgers compare
badgers upload
badgers report github
```

MVP에서는 GitHub Action에서 `badgers run` 하나로 대부분 처리하게 한다.

고급 사용자는 이미 생성된 LCOV 파일을 넘길 수 있게 한다.

```yaml
with:
  rust-lcov: target/llvm-cov/lcov.info
  python-lcov: coverage.lcov
```

## MVP 범위

1차 MVP:

- Rust line coverage
- Python line coverage
- LCOV parser
- coverage snapshot 생성
- GitHub repository storage branch push/fetch
- default branch latest baseline
- PR 전체 coverage 증감
- PR diff coverage
- PR comment
- GitHub Check Run

2차:

- matrix job report 병합
- monorepo path filter
- exclude/include 설정
- badge SVG 생성
- branch history
- HTML report 업로드
- dashboard 없이 storage branch 기반 history 조회

3차:

- 웹 UI
- Postgres 또는 BigQuery index
- Slack/webhook notification
- branch coverage
- file-level annotation
- GitLab 지원
- self-hosted storage adapters

## Storage adapter 전략

durable storage는 GitHub repository branch 하나로 통일한다. CLI의 storage 연산(`badgers snapshot push`, `badgers baseline fetch`)은 `--local-dir`로 넘긴 storage branch checkout 디렉터리에 대해 동작하고 (`--prefix`, `--repo`로 경로 결정), clone/commit/push는 action이 git으로 처리한다.

현재:

```text
local (storage branch checkout)
```

후속:

```text
s3-compatible
github-artifacts
postgres + object storage
```

GitHub artifacts backend는 demo 또는 작은 저장소용으로만 둔다. 장기 히스토리용 기본값으로 추천하지 않는다.

## 주요 리스크

- **Fork PR 제약**: fork에서 온 `pull_request` 이벤트는 `GITHUB_TOKEN`이 read-only라 storage branch push와 `pull-requests: write`(comment)가 동작하지 않는다. 현재는 fork PR에서 storage write를 건너뛰고 baseline 읽기만 한다. 오픈소스화 시 fork PR 처리 전략(`workflow_run` 2단계 패턴 등)이 필수다. `pull_request_target`은 보안 리스크가 크므로 기본값으로 쓰지 않는다.
- Python subprocess/multiprocessing coverage 설정이 누락될 수 있다.
- Rust workspace와 Python package가 섞인 monorepo에서 path normalization이 중요하다.
- 여러 matrix job에서 생성된 partial report를 안정적으로 병합해야 한다.
- generated files, migration files, test files 제외 정책을 명확히 해야 한다.
- PR comment가 매 push마다 중복 생성되지 않도록 marker 기반 update가 필요하다.
- storage branch 안의 경로를 repo, branch, PR, commit 기준으로 안정적으로 설계해야 한다.
- base branch 동시 push 시 storage branch push 경합과 `latest.json` 포인터 순서 처리가 필요하다.
- shallow clone(`fetch-depth: 1`) 환경에서는 merge-base 계산과 three-dot diff가 실패하므로 checkout 설정 가이드가 필요하다.

## 현재 결정 사항

- 프로젝트명: **Badgers**
- repo명: `badgers`
- 구현 언어: Rust
- 지원 언어: Rust, Python
- coverage 입력 포맷: LCOV 우선, JSON 후속
- 기본 storage: GitHub repository 전용 branch (`github-storage-branch`, 기본 `badgers-coverage`)
- storage 인증: `github-storage-token` (기본 `github.token`, 다른 repository는 GitHub App installation token 또는 fine-grained PAT)
- GitHub artifact storage: 기본 backend로 사용하지 않음
- 1차 목표: PR별 line coverage, coverage 증감, diff coverage

## 참고 링크

- [GitHub Actions limits](https://docs.github.com/en/actions/reference/limits)
- [GitHub Actions billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions)
- [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov)
- [coverage.py JSON report](https://coverage.readthedocs.io/en/latest/commands/cmd_json.html)
- [coverage.py LCOV report](https://coverage.readthedocs.io/en/latest/commands/cmd_lcov.html)
