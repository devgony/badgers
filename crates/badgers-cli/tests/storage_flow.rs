use assert_cmd::Command;
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
fn baseline_fetch_prefers_exact_then_pointer_then_none() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let out = tmp.path().join("base.json");

    let fetch = |merge_base: &str, out: &std::path::Path| {
        let mut cmd = badgers();
        cmd.args([
            "baseline",
            "fetch",
            "--merge-base",
            merge_base,
            "--base-ref",
            "main",
        ])
        .arg("-o")
        .arg(out)
        .arg("--local-dir")
        .arg(&store)
        .args(["--repo", "owner/repo"]);
        cmd
    };

    fetch("a1", &out)
        .assert()
        .success()
        .stdout(predicate::str::contains("baseline-kind=none"));
    assert!(!out.exists());

    let snapshot = write_snapshot(tmp.path(), "a1");
    push(&store, &snapshot, "a1", "2026-07-19T00:00:00Z");

    fetch("a1", &out).assert().success().stdout(
        predicate::str::contains("baseline-kind=exact")
            .and(predicate::str::contains("baseline-sha=a1")),
    );
    let fetched: serde_json::Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(fetched["commit_sha"], "a1");
    assert_eq!(fetched["files"][0]["path"], "pkg/calc.py");

    fetch("zzz-not-stored", &out).assert().success().stdout(
        predicate::str::contains("baseline-kind=approximate")
            .and(predicate::str::contains("baseline-sha=a1")),
    );
}
