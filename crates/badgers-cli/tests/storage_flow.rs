use assert_cmd::Command;
use badge_rs_storage::{BranchPointer, Keys, POINTER_SCHEMA_VERSION};
use predicates::prelude::*;

fn badgers() -> Command {
    Command::cargo_bin("badgers").unwrap()
}

fn write_snapshot(dir: &std::path::Path, sha: &str) -> std::path::PathBuf {
    let path = dir.join(format!("snapshot-{sha}.json"));
    let json = serde_json::json!({
        "schema_version": 1,
        "repo": "owner/repo",
        "commit_sha": sha,
        "branch": "main",
        "pr_number": null,
        "generated_at": "2026-07-19T00:00:00Z",
        "tool_versions": { "badgers": "0.1.0", "cargo_llvm_cov": null, "coverage_py": "7.6.1" },
        "files": [
            { "path": "pkg/calc.py", "language": "python",
              "line_hits": [ { "line": 1, "hits": 1 }, { "line": 2, "hits": 0 } ] }
        ]
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&json).unwrap()).unwrap();
    path
}

fn push(store: &std::path::Path, snapshot: &std::path::Path, sha: &str, committed_at: &str) {
    badgers()
        .args(["snapshot", "push"])
        .arg("--snapshot")
        .arg(snapshot)
        .args([
            "--sha",
            sha,
            "--committed-at",
            committed_at,
            "--branch",
            "main",
        ])
        .arg("--local-dir")
        .arg(store)
        .args(["--repo", "owner/repo"])
        .assert()
        .success();
}

fn write_store_object(store: &std::path::Path, key: &str, data: &[u8]) {
    let path = store.join(key);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, data).unwrap();
}

fn branch_pointer(sha: &str, snapshot_key: String) -> BranchPointer {
    BranchPointer {
        schema_version: POINTER_SCHEMA_VERSION,
        branch: "main".to_string(),
        commit_sha: sha.to_string(),
        committed_at: "2026-07-19T00:00:00Z".to_string(),
        snapshot_key,
        comparison_key: None,
        report_key: None,
        html_prefix: None,
        updated_at: "2026-07-19T00:01:00Z".to_string(),
    }
}

fn write_branch_pointer(store: &std::path::Path, pointer: &BranchPointer) {
    let key = Keys::new("badgers", "owner/repo").branch_pointer("main");
    write_store_object(store, &key, &serde_json::to_vec(pointer).unwrap());
}

fn fetch_baseline(store: &std::path::Path, merge_base: &str, output: &std::path::Path) -> Command {
    let mut command = badgers();
    command
        .args([
            "baseline",
            "fetch",
            "--merge-base",
            merge_base,
            "--base-ref",
            "main",
        ])
        .arg("-o")
        .arg(output)
        .arg("--local-dir")
        .arg(store)
        .args(["--repo", "owner/repo"]);
    command
}

#[test]
fn push_uploads_snapshot_and_advances_pointer() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let snapshot = write_snapshot(tmp.path(), "a1");

    push(&store, &snapshot, "a1", "2026-07-19T10:00:00+09:00");

    assert!(
        store
            .join("badgers/repos/owner/repo/commits/a1/coverage.json.zst")
            .is_file()
    );
    let pointer: serde_json::Value = serde_json::from_slice(
        &std::fs::read(store.join("badgers/repos/owner/repo/refs/main/latest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(pointer["commit_sha"], "a1");

    let older = write_snapshot(tmp.path(), "a0");
    push(&store, &older, "a0", "2026-07-18T10:00:00+09:00");
    let pointer: serde_json::Value = serde_json::from_slice(
        &std::fs::read(store.join("badgers/repos/owner/repo/refs/main/latest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(pointer["commit_sha"], "a1", "older run must not roll back");
}

#[test]
fn push_updates_pr_pointer() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let snapshot = write_snapshot(tmp.path(), "b1");

    badgers()
        .args(["snapshot", "push"])
        .arg("--snapshot")
        .arg(&snapshot)
        .args([
            "--sha",
            "b1",
            "--committed-at",
            "2026-07-19T00:00:00Z",
            "--pr",
            "547",
        ])
        .arg("--local-dir")
        .arg(&store)
        .args(["--repo", "owner/repo"])
        .assert()
        .success();

    assert!(
        store
            .join("badgers/repos/owner/repo/prs/547/latest.json")
            .is_file()
    );
}

#[test]
fn push_stores_comparison_and_only_updates_latest_report_for_newer_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let comparison = tmp.path().join("comparison.json");
    let report = tmp.path().join("README.md");
    let comparison_json = serde_json::to_vec_pretty(&serde_json::json!({
        "schema_version": 1,
        "head_sha": "new",
        "base_sha": null,
        "comparison": { "base_available": false, "files": [] }
    }))
    .unwrap();
    std::fs::write(&comparison, &comparison_json).unwrap();
    std::fs::write(&report, "# New report\n").unwrap();

    let newest = write_snapshot(tmp.path(), "new");
    badgers()
        .args(["snapshot", "push"])
        .arg("--snapshot")
        .arg(&newest)
        .arg("--comparison")
        .arg(&comparison)
        .arg("--report")
        .arg(&report)
        .args([
            "--sha",
            "new",
            "--committed-at",
            "2026-07-19T10:00:00Z",
            "--branch",
            "main",
        ])
        .arg("--local-dir")
        .arg(&store)
        .args(["--repo", "owner/repo"])
        .assert()
        .success();

    let root = store.join("badgers/repos/owner/repo");
    assert_eq!(
        std::fs::read_to_string(root.join("commits/new/README.md")).unwrap(),
        "# New report\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("refs/main/README.md")).unwrap(),
        "# New report\n"
    );
    let compressed = std::fs::read(root.join("commits/new/comparison.json.zst")).unwrap();
    assert_eq!(
        zstd::decode_all(compressed.as_slice()).unwrap(),
        comparison_json
    );
    let pointer: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("refs/main/latest.json")).unwrap())
            .unwrap();
    assert_eq!(
        pointer["comparison_key"],
        "badgers/repos/owner/repo/commits/new/comparison.json.zst"
    );
    assert_eq!(
        pointer["report_key"],
        "badgers/repos/owner/repo/commits/new/README.md"
    );

    std::fs::write(&report, "# Old report\n").unwrap();
    let older = write_snapshot(tmp.path(), "old");
    badgers()
        .args(["snapshot", "push"])
        .arg("--snapshot")
        .arg(&older)
        .arg("--report")
        .arg(&report)
        .args([
            "--sha",
            "old",
            "--committed-at",
            "2026-07-18T10:00:00Z",
            "--branch",
            "main",
        ])
        .arg("--local-dir")
        .arg(&store)
        .args(["--repo", "owner/repo"])
        .assert()
        .success();

    assert_eq!(
        std::fs::read_to_string(root.join("refs/main/README.md")).unwrap(),
        "# New report\n",
        "older snapshots must not replace the latest readable report"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("commits/old/README.md")).unwrap(),
        "# Old report\n"
    );
}

#[test]
fn push_rejects_snapshot_sha_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let snapshot = write_snapshot(tmp.path(), "actual");
    badgers()
        .args(["snapshot", "push"])
        .arg("--snapshot")
        .arg(snapshot)
        .args([
            "--sha",
            "different",
            "--committed-at",
            "2026-07-19T10:00:00Z",
        ])
        .arg("--local-dir")
        .arg(tmp.path().join("store"))
        .args(["--repo", "owner/repo"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "snapshot commit SHA actual does not match --sha different",
        ));
}

#[test]
fn push_rejects_comparison_sha_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let snapshot = write_snapshot(tmp.path(), "actual");
    let comparison = tmp.path().join("comparison.json");
    std::fs::write(
        &comparison,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "head_sha": "different",
            "base_sha": null,
            "comparison": { "base_available": false, "files": [] }
        }))
        .unwrap(),
    )
    .unwrap();
    badgers()
        .args(["snapshot", "push"])
        .arg("--snapshot")
        .arg(snapshot)
        .arg("--comparison")
        .arg(comparison)
        .args(["--sha", "actual", "--committed-at", "2026-07-19T10:00:00Z"])
        .arg("--local-dir")
        .arg(tmp.path().join("store"))
        .args(["--repo", "owner/repo"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "comparison head SHA different does not match --sha actual",
        ));
}

#[test]
fn baseline_fetch_prefers_exact_then_pointer_then_none() {
    const SHA: &str = "a100000000000000000000000000000000000000";
    const MISSING_SHA: &str = "b100000000000000000000000000000000000000";
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let out = tmp.path().join("base.json");

    fetch_baseline(&store, MISSING_SHA, &out)
        .assert()
        .success()
        .stdout(predicate::str::contains("baseline-kind=none"));
    assert!(!out.exists());

    let snapshot = write_snapshot(tmp.path(), SHA);
    push(&store, &snapshot, SHA, "2026-07-19T00:00:00Z");

    fetch_baseline(&store, SHA, &out).assert().success().stdout(
        predicate::str::contains("baseline-kind=exact")
            .and(predicate::str::contains(format!("baseline-sha={SHA}"))),
    );
    let fetched: serde_json::Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(fetched["commit_sha"], SHA);
    assert_eq!(fetched["files"][0]["path"], "pkg/calc.py");

    fetch_baseline(&store, MISSING_SHA, &out)
        .assert()
        .success()
        .stdout(
            predicate::str::contains("baseline-kind=approximate")
                .and(predicate::str::contains(format!("baseline-sha={SHA}"))),
        );
}

#[test]
fn baseline_fetch_rejects_unsupported_pointer_schema_version() {
    const MISSING_SHA: &str = "b100000000000000000000000000000000000000";
    const POINTER_SHA: &str = "a100000000000000000000000000000000000000";
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let output = tmp.path().join("base.json");
    let keys = Keys::new("badgers", "owner/repo");
    let mut pointer = branch_pointer(POINTER_SHA, keys.commit_snapshot(POINTER_SHA));
    pointer.schema_version = POINTER_SCHEMA_VERSION + 1;
    write_branch_pointer(&store, &pointer);

    fetch_baseline(&store, MISSING_SHA, &output)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "unsupported pointer schema version 2",
        ));
    assert!(!output.exists());
}

#[test]
fn baseline_fetch_rejects_pointer_for_another_branch() {
    const MISSING_SHA: &str = "b100000000000000000000000000000000000000";
    const POINTER_SHA: &str = "a100000000000000000000000000000000000000";
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let output = tmp.path().join("base.json");
    let keys = Keys::new("badgers", "owner/repo");
    let mut pointer = branch_pointer(POINTER_SHA, keys.commit_snapshot(POINTER_SHA));
    pointer.branch = "release".to_string();
    write_branch_pointer(&store, &pointer);

    fetch_baseline(&store, MISSING_SHA, &output)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "belongs to branch \"release\", not \"main\"",
        ));
    assert!(!output.exists());
}

#[test]
fn baseline_fetch_rejects_pointer_snapshot_for_another_commit() {
    const MISSING_SHA: &str = "b100000000000000000000000000000000000000";
    const POINTER_SHA: &str = "a100000000000000000000000000000000000000";
    const SNAPSHOT_SHA: &str = "c100000000000000000000000000000000000000";
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let output = tmp.path().join("base.json");
    let keys = Keys::new("badgers", "owner/repo");
    let pointer = branch_pointer(POINTER_SHA, keys.commit_snapshot(SNAPSHOT_SHA));
    write_branch_pointer(&store, &pointer);

    fetch_baseline(&store, MISSING_SHA, &output)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "snapshot path does not match commit",
        ));
    assert!(!output.exists());
}

#[test]
fn baseline_fetch_rejects_snapshot_with_mismatched_metadata() {
    const SHA: &str = "a100000000000000000000000000000000000000";
    const OTHER_SHA: &str = "b100000000000000000000000000000000000000";
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let output = tmp.path().join("base.json");
    let snapshot = write_snapshot(tmp.path(), OTHER_SHA);
    let compressed = zstd::encode_all(std::fs::read(snapshot).unwrap().as_slice(), 3).unwrap();
    let key = Keys::new("badgers", "owner/repo").commit_snapshot(SHA);
    write_store_object(&store, &key, &compressed);

    fetch_baseline(&store, SHA, &output)
        .assert()
        .failure()
        .stderr(predicate::str::contains("stored coverage snapshot commit"));
    assert!(!output.exists());
}

#[cfg(unix)]
#[test]
fn baseline_fetch_rejects_symlinked_pointer_snapshot_path() {
    const MISSING_SHA: &str = "b100000000000000000000000000000000000000";
    const POINTER_SHA: &str = "a100000000000000000000000000000000000000";
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let outside = tmp.path().join("outside");
    let output = tmp.path().join("base.json");
    let keys = Keys::new("badgers", "owner/repo");
    let snapshot_key = keys.commit_snapshot(POINTER_SHA);
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("coverage.json.zst"), b"not a snapshot").unwrap();
    let linked = store.join(format!("badgers/repos/owner/repo/commits/{POINTER_SHA}"));
    std::fs::create_dir_all(linked.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &linked).unwrap();
    write_branch_pointer(&store, &branch_pointer(POINTER_SHA, snapshot_key));

    fetch_baseline(&store, MISSING_SHA, &output)
        .assert()
        .failure()
        .stderr(predicate::str::contains("symlink"));
    assert!(!output.exists());
}

#[test]
fn baseline_fetch_decodes_compressed_pointer_snapshot_from_local_checkout() {
    const MISSING_SHA: &str = "b100000000000000000000000000000000000000";
    const POINTER_SHA: &str = "a100000000000000000000000000000000000000";
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let output = tmp.path().join("base.json");
    let snapshot = write_snapshot(tmp.path(), POINTER_SHA);
    let snapshot_json = std::fs::read(snapshot).unwrap();
    let compressed = zstd::encode_all(snapshot_json.as_slice(), 3).unwrap();
    let keys = Keys::new("badgers", "owner/repo");
    let snapshot_key = keys.commit_snapshot(POINTER_SHA);
    write_store_object(&store, &snapshot_key, &compressed);
    write_branch_pointer(&store, &branch_pointer(POINTER_SHA, snapshot_key.clone()));

    fetch_baseline(&store, MISSING_SHA, &output)
        .assert()
        .success()
        .stdout(predicate::str::contains("baseline-kind=approximate").and(
            predicate::str::contains(format!("baseline-sha={POINTER_SHA}")),
        ));
    assert_eq!(std::fs::read(output).unwrap(), snapshot_json);
}

#[test]
fn push_stores_html_bundle_and_records_html_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let snapshot = write_snapshot(tmp.path(), "c1");

    let report_dir = tmp.path().join("coverage-report");
    std::fs::create_dir_all(report_dir.join("assets")).unwrap();
    std::fs::write(report_dir.join("index.html"), b"<html></html>").unwrap();
    std::fs::write(report_dir.join("assets").join("style.css"), b"body{}").unwrap();

    badgers()
        .args(["snapshot", "push"])
        .arg("--snapshot")
        .arg(&snapshot)
        .arg("--html-report")
        .arg(&report_dir)
        .args([
            "--sha",
            "c1",
            "--committed-at",
            "2026-07-19T10:00:00Z",
            "--branch",
            "main",
        ])
        .arg("--local-dir")
        .arg(&store)
        .args(["--repo", "owner/repo"])
        .assert()
        .success();

    let root = store.join("badgers/repos/owner/repo");
    assert!(root.join("commits/c1/html/index.html").is_file());
    assert!(root.join("commits/c1/html/assets/style.css").is_file());
    assert_eq!(
        std::fs::read(root.join("commits/c1/html/index.html")).unwrap(),
        b"<html></html>"
    );

    let pointer: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("refs/main/latest.json")).unwrap())
            .unwrap();
    assert_eq!(
        pointer["html_prefix"],
        "badgers/repos/owner/repo/commits/c1/html"
    );
}

#[cfg(unix)]
#[test]
fn push_rejects_symlink_inside_html_report() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let snapshot = write_snapshot(tmp.path(), "d1");

    let report_dir = tmp.path().join("coverage-report");
    std::fs::create_dir_all(&report_dir).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", report_dir.join("evil.html")).unwrap();

    badgers()
        .args(["snapshot", "push"])
        .arg("--snapshot")
        .arg(&snapshot)
        .arg("--html-report")
        .arg(&report_dir)
        .args([
            "--sha",
            "d1",
            "--committed-at",
            "2026-07-19T10:00:00Z",
            "--branch",
            "main",
        ])
        .arg("--local-dir")
        .arg(&store)
        .args(["--repo", "owner/repo"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("symlink"));
}

#[cfg(unix)]
#[test]
fn push_rejects_backslash_in_html_filename() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let snapshot = write_snapshot(tmp.path(), "e1");

    let report_dir = tmp.path().join("coverage-report");
    std::fs::create_dir_all(&report_dir).unwrap();
    std::fs::write(report_dir.join("bad\\name.html"), b"x").unwrap();

    badgers()
        .args(["snapshot", "push"])
        .arg("--snapshot")
        .arg(&snapshot)
        .arg("--html-report")
        .arg(&report_dir)
        .args([
            "--sha",
            "e1",
            "--committed-at",
            "2026-07-19T10:00:00Z",
            "--branch",
            "main",
        ])
        .arg("--local-dir")
        .arg(&store)
        .args(["--repo", "owner/repo"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unsafe HTML report filename"));
}

#[test]
fn baseline_fetch_rejects_invalid_merge_base_sha() {
    let tmp = tempfile::tempdir().unwrap();
    badgers()
        .args([
            "baseline",
            "fetch",
            "--merge-base",
            "bad\nname=value",
            "--base-ref",
            "main",
        ])
        .arg("-o")
        .arg(tmp.path().join("base.json"))
        .arg("--local-dir")
        .arg(tmp.path().join("store"))
        .args(["--repo", "owner/repo"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid merge-base commit SHA"));
}
