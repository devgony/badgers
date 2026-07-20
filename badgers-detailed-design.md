# Badgers 상세 설계

작성일: 2026-07-04
상태: Draft v1
선행 문서: [badgers-project-summary.md](./badgers-project-summary.md)

## 1. 개요

Badgers는 Rust/Python 프로젝트의 line coverage를 PR마다 계산하고, baseline 대비 증감과 diff coverage를 GitHub PR에 리포트하는 도구다. 이 문서는 프로젝트 요약에서 결정된 사항을 바탕으로 MVP(1차) 구현의 상세 설계를 기술한다.

### 1.1 목표 (MVP)

- Rust(`cargo llvm-cov`)와 Python(`coverage.py`)의 LCOV 출력을 공통 모델로 정규화
- PR head snapshot과 baseline snapshot의 전체 coverage 증감 계산
- `git diff base...head` 기준 diff coverage 계산
- GitHub Check Run + marker 기반 PR comment 리포트
- GCS에 snapshot 저장/조회

### 1.2 비목표 (MVP에서 제외)

- Branch coverage, function coverage
- 웹 UI, dashboard, badge SVG
- matrix job 병합 (2차)
- fork PR 지원 (오픈소스화 시점에 `workflow_run` 패턴으로 해결)
- GitHub artifacts / S3 backend
- coverage threshold 기반 CI 실패 처리 (soft-fail만 지원, hard-fail은 2차)

---

## 2. 아키텍처

```text
+--------------------------------------------------------------+
| GitHub Actions (badgers-action)                              |
|                                                              |
|  actions/checkout (fetch-depth: 0)                           |
|  google-github-actions/auth (WIF)                            |
|  badgers CLI                                                 |
|   |                                                          |
|   |  collect        merge          compare        report     |
|   v                                                          |
|  [lcov parse] -> [CoverageSnapshot] -> [Delta/DiffCov] ------+--> GitHub API
|                        |                    ^                |    (Check Run,
|                        v                    |                |     PR comment)
|                   GCS upload           GCS download          |
+------------------------|--------------------|----------------+
                         v                    |
                 gs://bucket/badgers/... <----+
```

구성 요소:

| 구성 요소 | 역할 |
|---|---|
| `badgers` CLI (Rust) | 수집, 파싱, 병합, 비교, 업로드, 리포트의 전 과정 |
| `badgers-action` | CLI 바이너리를 받아 실행하는 GitHub Action 래퍼 |
| GCS | snapshot 원장 (source of truth) |
| GitHub API | Check Run 생성, PR comment 작성/갱신 |

서버 컴포넌트는 없다. 모든 로직은 CI job 안에서 실행된다 (stateless, storage가 유일한 상태).

### 2.1 Cargo workspace 구조

```text
badgers/
  Cargo.toml            # workspace root
  crates/
    badgers-core/       # 데이터 모델, snapshot 병합/비교, diff coverage 계산
    badgers-lcov/       # LCOV parser/writer
    badgers-storage/    # StorageBackend trait + gcs/local 구현
    badgers-github/     # Check Run, PR comment, GitHub context 파싱
    badgers-cli/        # clap 기반 CLI (binary: badgers)
  action/               # badgers-action (composite action)
  docs/
```

의존 방향: `cli -> {core, lcov, storage, github}`, `storage/github/lcov -> core`. `core`는 외부 I/O 의존성이 없다 (순수 로직, 테스트 용이).

주요 dependency:

- `clap` (derive) - CLI
- `serde` + `serde_json` - 직렬화
- `zstd` - snapshot 압축
- `reqwest` (rustls) - GCS/GitHub HTTP
- `tokio` - async runtime
- `jiff` 또는 `time` - RFC 3339 timestamp
- `thiserror` / `anyhow` - 에러 처리
- GCS는 공식 SDK 대신 JSON API 직접 호출 (필요 API가 object get/put/list 정도로 좁고, WIF 토큰은 action이 미리 발급해 `GOOGLE_APPLICATION_CREDENTIALS` 또는 access token 환경변수로 전달됨)

---

## 3. 데이터 모델

### 3.1 CoverageSnapshot (schema_version = 1)

```rust
/// 하나의 commit에 대한 coverage 측정 결과 전체.
/// GCS에 coverage.json.zst로 저장되는 canonical 산출물.
struct CoverageSnapshot {
    schema_version: u32,          // 1
    repo: String,                 // "jubilee-works/timetree-planner-server"
    commit_sha: String,           // full 40-char SHA
    branch: Option<String>,       // push 이벤트일 때
    pr_number: Option<u64>,       // pull_request 이벤트일 때
    generated_at: String,         // RFC 3339 UTC, 예: "2026-07-04T02:00:00Z"
    tool_versions: ToolVersions,  // 재현성/디버깅용
    files: Vec<FileCoverage>,     // path 오름차순 정렬 (deterministic output)
}

struct ToolVersions {
    badgers: String,              // CLI 버전
    cargo_llvm_cov: Option<String>,
    coverage_py: Option<String>,
}

struct FileCoverage {
    path: String,                 // repo root 기준 상대 경로, '/' 구분자로 정규화
    language: Language,
    line_hits: Vec<LineHit>,      // line 오름차순, canonical source
}

struct LineHit {
    line: u32,                    // 1-based
    hits: u64,                    // LCOV DA 카운트 (u32 overflow 방지 위해 u64)
}

enum Language { Rust, Python, Unknown }
```

설계 결정:

- **`executable_lines`/`covered_lines`를 struct에서 제거**하고 메서드로 유도한다 (`fn executable_lines(&self) -> u32 { self.line_hits.len() }`). 저장 데이터에 중복 필드를 두면 불일치 버그의 원천이 된다. 요약 문서의 초안 모델에서 변경된 부분.
- `files`와 `line_hits`는 정렬 보장 → snapshot 비교와 diff가 결정적(deterministic)이고, 같은 입력이면 byte-identical 출력이 나온다.
- percentage는 저장하지 않는다. 항상 `covered/executable` 정수 쌍에서 계산한다 (부동소수점 누적 오차 방지).
- `schema_version` 정책: 필드 추가는 minor(버전 유지, serde default), 의미 변경/제거는 version bump. 읽기 시 자신보다 높은 version은 에러, 낮은 version은 migration 함수로 변환.

### 3.2 Manifest

run 단위 메타데이터. 스냅샷 생성 컨텍스트를 기록한다.

```rust
struct RunManifest {
    schema_version: u32,
    run_id: u64,                  // GITHUB_RUN_ID
    run_attempt: u32,
    repo: String,
    commit_sha: String,
    event: String,                // "pull_request" | "push"
    snapshot_key: String,         // GCS object key
    created_at: String,
}
```

### 3.3 BranchPointer (`refs/{encoded_branch}/latest.json`)

```rust
struct BranchPointer {
    schema_version: u32,
    branch: String,               // 원본 branch 이름 (인코딩 전)
    commit_sha: String,
    committed_at: String,         // git commit timestamp (포인터 갱신 순서 판단용)
    snapshot_key: String,
    updated_at: String,
}
```

### 3.4 percentage 계산 규칙

```text
coverage_pct = covered_lines / executable_lines        (executable_lines == 0이면 None)
delta        = head_pct - base_pct                      (둘 다 Some일 때만)
표시 정밀도  = 소수점 둘째 자리, 내부 계산은 정수 쌍 유지
```

`executable_lines == 0`인 snapshot(예: 코드 없는 repo)은 "no measurable lines"로 표시하고 0%나 100%로 강제하지 않는다.

---

## 4. Coverage 수집 파이프라인

### 4.1 Rust

```bash
cargo llvm-cov --workspace --lcov --output-path "$BADGERS_TMP/rust.lcov"
```

- `badgers collect rust`는 위 명령을 실행하거나, `--lcov-file` 옵션으로 기존 파일을 받는다.
- 옵션 passthrough: `badgers collect rust -- --features foo` 형태로 cargo llvm-cov에 추가 인자 전달.

### 4.2 Python

```bash
coverage run -m pytest
coverage combine          # parallel/subprocess 데이터 존재 시
coverage lcov -o "$BADGERS_TMP/python.lcov"
```

- **coverage.py >= 6.3 필수** (`coverage lcov` 지원 버전). CLI가 `coverage --version`을 확인하고 미달이면 명확한 에러를 낸다.
- subprocess coverage를 위해 `.coveragerc`/`pyproject.toml`에 `parallel = true`, `concurrency` 설정을 권장하는 문서를 제공한다 (Badgers가 자동 설정하지는 않음 - 프로젝트 소유 설정을 건드리지 않는다).

### 4.3 LCOV 파싱 (`badgers-lcov`)

MVP에서 사용하는 LCOV 레코드:

| 레코드 | 의미 | 처리 |
|---|---|---|
| `SF:<path>` | 파일 시작 | path 정규화 진입점 |
| `DA:<line>,<hits>` | line 실행 카운트 | `LineHit`로 변환 |
| `LF:`/`LH:` | 파일 요약 | 파싱 후 DA 합계와 대조 검증 (불일치 시 warning) |
| `end_of_record` | 파일 종료 | |
| `FN`/`FNDA`/`BRDA` 등 | function/branch | MVP에서 무시 (skip, 에러 아님) |

같은 파일에 대한 `DA` 중복(예: 여러 test binary가 같은 파일 커버)은 **hits 합산**으로 병합한다.

### 4.4 Path normalization

LCOV의 `SF:` 경로를 repo-relative로 통일하는 것이 언어 병합의 핵심이다.

규칙 (순서대로 적용):

1. 절대 경로면 repo root(현재 작업 디렉토리 또는 `--repo-root`)를 prefix strip
2. `\` → `/` 변환
3. `./` prefix 제거, `..` 세그먼트는 lexical하게 해소
4. repo root 밖을 가리키는 경로 (`/usr/lib/python3/...`, `~/.cargo/registry/...`)는 **third-party로 간주하고 drop** (warning 로그)
5. 결과가 실제 checkout에 존재하는지 확인. 없으면 warning 후 유지 (generated file일 수 있음)

### 4.5 병합 (`badgers merge`)

- 입력: 언어별 파싱 결과 N개
- 같은 path가 두 언어에서 나오면 에러 (정상 상황에서 불가능 - 설정 오류 신호)
- 출력: 단일 `CoverageSnapshot` (files 정렬)

---

## 5. Baseline 결정 알고리즘

```text
fn resolve_baseline(base_ref, head_ref, storage) -> Baseline:
    merge_base = git merge-base origin/{base_ref} {head_sha}

    # 1순위: merge-base commit의 snapshot
    if storage.exists(commits/{merge_base}/coverage.json.zst):
        return Baseline::Exact(merge_base)

    # 2순위: base branch 최신 성공 snapshot (근사 baseline)
    pointer = storage.get(refs/{encode(base_ref)}/latest.json)
    if pointer is Some:
        return Baseline::Approximate(pointer.commit_sha)

    # 3순위: baseline 없음
    return Baseline::None
```

- `Approximate`일 때 리포트에 `baseline: abc1234 (approximate - merge-base snapshot not found)`를 명시한다. base branch에 PR 분기 이후의 커밋이 섞여 증감이 부정확할 수 있음을 사용자에게 알린다.
- `None`일 때는 delta를 생략하고 절대값만 리포트하며, base branch workflow를 먼저 실행하라는 안내를 comment에 포함한다.
- shallow clone에서 merge-base 계산이 실패하면 명확한 에러: `"fetch-depth: 0 (or sufficient depth) is required"`.

### 5.1 Base branch push 시 포인터 갱신 (동시성)

```text
1. commits/{sha}/coverage.json.zst 업로드 (무조건, 충돌 없음 - sha가 key)
2. refs/{branch}/latest.json 읽기 (object generation 번호 확보)
3. 기존 pointer.committed_at >= 새 commit의 committed_at 이면 갱신 스킵
   (재실행된 오래된 workflow가 최신 포인터를 되돌리는 것 방지)
4. ifGenerationMatch={generation}으로 conditional write
5. 412 Precondition Failed 시 2번부터 재시도 (최대 3회, jitter backoff)
```

### 5.2 Branch 이름 인코딩

GCS key에 들어가는 branch 이름은 다음으로 인코딩한다:

```text
encode(branch) = percent-encode(branch, unreserved = [A-Za-z0-9._-])
예: "release/1.2" -> "release%2F1.2"
```

percent-encoding은 가역적이고 충돌이 없다 (`__` 치환 방식은 `a/b`와 `a__b` 충돌 가능성이 있어 배제).

---

## 6. Diff Coverage 알고리즘

### 6.1 변경 라인 추출

```bash
git diff --no-color --unified=0 --diff-filter=ACMR \
    $(git merge-base origin/$BASE_REF HEAD)...HEAD
```

- three-dot이 아닌 **merge-base 대비 two-dot** 명시 (three-dot과 동치이지만 의도가 명확)
- `--diff-filter=ACMR`: Added/Copied/Modified/Renamed만. 삭제 파일은 diff coverage 대상 없음
- `--unified=0`: hunk header(`@@ -a,b +c,d @@`)에서 **new file 기준 라인 번호**만 추출
- rename(`R`)은 새 경로 기준으로 처리. 유사도 감지는 git 기본값 사용

### 6.2 계산

```text
for file in changed_files:
    changed = set(added/modified line numbers in new file)
    fc = head_snapshot.file(file)          # 없으면 skip (측정 대상 아닌 파일)
    executable = { lh.line for lh in fc.line_hits }
    relevant   = changed ∩ executable       # 주석/공백/비실행 라인 제외
    covered    = { l in relevant where hits > 0 }

diff_coverage = |covered 전체| / |relevant 전체|    (relevant == 0이면 None → "no measurable changed lines")
```

- head snapshot에 없는 파일(docs, config 등)은 diff coverage 분모에 넣지 않는다.
- uncovered line 목록은 파일별로 최대 N개(기본 10)까지 comment에 표기하고 나머지는 개수만 표시.

---

## 7. Storage 설계

### 7.1 StorageBackend trait

```rust
#[async_trait]
trait StorageBackend {
    async fn get(&self, key: &str) -> Result<Option<Bytes>, StorageError>;
    async fn put(&self, key: &str, data: Bytes, opts: PutOptions) -> Result<(), StorageError>;
    async fn exists(&self, key: &str) -> Result<bool, StorageError>;
}

struct PutOptions {
    if_generation_match: Option<i64>,  // GCS conditional write. local backend는 무시
    content_type: &'static str,
}
```

MVP 구현체: `GcsBackend`, `LocalBackend`(파일시스템 - 테스트/로컬 개발용).

### 7.2 Object key 스킴

```text
{prefix}/repos/{owner}/{repo}/
  commits/{sha}/coverage.json.zst      # canonical snapshot, 영구 보관
  commits/{sha}/lcov.info.zst          # 원본 LCOV (디버깅용), lifecycle 90d
  refs/{encoded_branch}/latest.json    # branch 포인터 (비압축 - 작음)
  prs/{pr_number}/latest.json          # PR 최신 run 포인터
  runs/{run_id}-{run_attempt}/manifest.json
```

- prefix 기본값: `badgers` (action input `gcs-prefix`로 override)
- 압축: zstd level 3 (속도/압축률 균형)
- lifecycle rule은 Badgers가 관리하지 않는다. 권장 설정을 문서로 제공 (`lcov.info.zst` 90일, `runs/` 180일)

### 7.3 GCS 인증

- action에서 `google-github-actions/auth@v3`(WIF)가 credential을 준비
- CLI는 ADC(Application Default Credentials) 체인을 따른다: `GOOGLE_APPLICATION_CREDENTIALS` → metadata server. 로컬 개발은 `gcloud auth application-default login`
- 필요 권한: bucket에 `roles/storage.objectUser` (object CRUD, bucket 관리 권한 없음)

---

## 8. GitHub 리포팅

### 8.1 PR comment (head commit marker 기반 upsert)

```text
1. issue comments에서 marker 검색: <!-- badgers-report:{owner}/{repo}:{pr_number}:{head_sha} -->
2. 같은 head commit의 marker가 있으면 PATCH (rerun update), 없으면 POST (new push create)
```

comment 형식:

```markdown
<!-- badgers-report:jubilee-works/timetree-planner-server:123:def5678901234567890123456789012345678901 -->
## 🦡 Badgers Coverage Report

| | Coverage | Δ |
|---|---|---|
| **Total** | 84.21% (3,412/4,052) | 🟢 +0.34% |
| **Diff** | 92.30% (24/26) | |

**Baseline**: `abc1234` (merge-base)
**Head**: `def5678`

<details><summary>Uncovered changed lines (2)</summary>

- `src/api/handler.rs`: L45, L82
</details>
```

### 8.2 Check Run

- name: `badgers/coverage`
- conclusion: MVP에서는 항상 `success` (delta가 음수여도) + summary에 수치 표기. threshold 기반 `failure`는 2차 (`fail-under-diff`, `fail-on-drop` input 예약)
- output.summary: comment와 동일한 markdown

### 8.3 필요 permissions

```yaml
permissions:
  contents: read        # checkout
  id-token: write       # WIF
  pull-requests: write  # comment
  checks: write         # check run
```

fork PR에서는 이 중 write 권한이 모두 무효화된다 (read-only token). MVP는 same-repo PR만 지원하며, CLI가 fork PR을 감지하면 (`head.repo != base.repo`) 리포팅을 skip하고 warning 후 exit 0 한다 (CI를 깨지 않음).

---

## 9. CLI 상세

```text
badgers run                     # collect + merge + compare + report + upload 전체 (MVP 기본 진입점)
badgers collect rust [--lcov-file <path>] [-- <cargo llvm-cov args>]
badgers collect python [--lcov-file <path>]
badgers merge <inputs...> -o snapshot.json
badgers compare --head <snapshot> [--base <snapshot>] [--diff <git range>]
badgers upload <snapshot>
badgers report github --comparison <file>
```

### 9.1 `badgers run`의 동작

1. GitHub context 파싱 (`GITHUB_EVENT_PATH`, `GITHUB_REPOSITORY`, `GITHUB_SHA` 등)
2. 언어 자동 감지: `Cargo.toml` 존재 → rust, `pyproject.toml`/`setup.py` 존재 → python (input으로 override 가능)
3. collect → merge → baseline resolve → compare → report → upload
4. 실패 정책: **coverage 수집 실패는 hard error** (exit != 0), **리포팅/업로드 실패는 기본 hard error, `--soft-fail`시 warning + exit 0**

### 9.2 설정 파일 (`badgers.toml`, 선택)

```toml
[coverage]
exclude = ["**/tests/**", "**/*_test.rs", "**/migrations/**"]

[rust]
args = ["--workspace"]

[python]
# coverage.py 설정은 프로젝트의 .coveragerc를 그대로 사용
```

exclude 패턴은 snapshot 생성 시점에 적용한다 (저장된 snapshot은 이미 필터링됨 → baseline과 head의 필터 기준이 commit별 설정을 따라가 일관성 유지).

### 9.3 Exit code

| code | 의미 |
|---|---|
| 0 | 성공 (baseline 없음, fork PR skip 포함) |
| 1 | 실행 오류 (수집 실패, 파싱 실패, 인증 실패) |
| 2 | 사용법 오류 |
| 3 | (예약) threshold 미달 - 2차 |

---

## 10. GitHub Action 설계

**composite action** + 사전 빌드된 바이너리 다운로드 방식 (docker action은 시작 오버헤드, JS action은 바이너리 래핑 보일러플레이트가 커서 배제).

```yaml
# action/action.yml
inputs:
  storage:        { default: "gcs" }
  gcs-bucket:     { required: true }
  gcs-prefix:     { default: "badgers" }
  rust-lcov:      { required: false }   # 기존 LCOV 재사용
  python-lcov:    { required: false }
  languages:      { required: false }   # "rust,python" - 자동 감지 override
  version:        { default: "latest" } # badgers CLI 버전
  soft-fail:      { default: "false" }
runs:
  using: composite
  steps:
    - run: <install badgers binary from GitHub Releases (버전 고정 + checksum 검증)>
    - run: badgers run ...
```

- 바이너리 배포: GitHub Releases에 `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` (musl static 우선 검토)
- action은 WIF 인증을 직접 하지 않는다. 사용자가 `google-github-actions/auth`를 먼저 실행하는 것을 전제 (문서화)

---

## 11. 에러 처리 원칙

- 네트워크(GCS/GitHub) 호출은 idempotent 연산에 한해 3회 retry (exponential backoff + jitter)
- LCOV 파싱: 알 수 없는 레코드는 skip, 구조 깨짐(잘린 파일 등)은 에러
- 부분 실패 허용 지점: baseline 조회 실패 → "baseline unavailable"로 계속 진행 (수집된 head snapshot 업로드는 수행)
- 모든 warning/error는 GitHub Actions annotation 형식(`::warning::`)으로도 출력

---

## 12. 테스트 전략

| 레이어 | 방법 |
|---|---|
| `badgers-core` | 순수 unit test. snapshot 비교/diff coverage는 table-driven test + proptest(정렬/병합 불변식) |
| `badgers-lcov` | 실제 cargo llvm-cov / coverage.py 출력물을 fixture로 고정 (`tests/fixtures/*.lcov`) |
| `badgers-storage` | `LocalBackend`로 trait 계약 테스트, GCS는 wiremock 기반 HTTP mock + precondition(412) 시나리오 |
| `badgers-github` | wiremock으로 comment upsert / check run 시나리오 |
| E2E | 별도 sandbox repo에서 실제 action 실행 (self-dogfooding: badgers repo 자신의 PR에 badgers 적용) |

---

## 13. 마일스톤

| 단계 | 산출물 | 검증 기준 |
|---|---|---|
| M1 | `badgers-core` + `badgers-lcov`: LCOV → snapshot → compare/diff-coverage | fixture 기반 unit test 통과 |
| M2 | `badgers-storage` (local, gcs) + baseline resolve + pointer 갱신 | mock GCS 통합 테스트, 412 재시도 검증 |
| M3 | `badgers-github` + `badgers run` end-to-end | 실 repo test PR에서 comment/check run 생성 확인 |
| M4 | `badgers-action` + 바이너리 릴리즈 파이프라인 | sandbox repo에서 action 동작, self-dogfooding 시작 |
| M5 | 사내 repo(timetree-planner-server) 적용 | 실 PR 2주 운영, false report 없음 |

---

## 14. 미해결 질문 (구현 전 결정 필요)

1. **snapshot 크기**: 대형 monorepo에서 `line_hits` 전체 저장 시 zstd 후 수 MB 예상. 문제가 되면 line bitmap 인코딩(covered/uncovered 2-bit) 검토. → M1에서 실측 후 결정
2. **PR head가 여러 번 push될 때 이전 commit snapshot 정리**: lifecycle rule에 맡길지, `prs/{n}/` 하위만 짧은 TTL을 둘지 → 운영하며 결정
3. **`merge_group` (merge queue) 이벤트 지원**: 사내 merge queue 도입 시점에 결정
4. **badge SVG의 serving 경로** (GCS public vs Cloud Run proxy): 2차 범위
