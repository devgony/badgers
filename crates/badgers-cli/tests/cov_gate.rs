use assert_cmd::Command;
use predicates::prelude::*;

fn init_repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join("tracked.txt"), "fixture\n").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Badgers Test"],
        vec!["config", "user.email", "badgers@example.com"],
        vec!["add", "tracked.txt"],
        vec!["commit", "-q", "-m", "fixture"],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(repo.path())
                .status()
                .unwrap()
                .success()
        );
    }
    repo
}

fn cov_all(
    repo: &tempfile::TempDir,
    lcov: &std::path::Path,
    extra: &[&str],
) -> assert_cmd::assert::Assert {
    Command::cargo_bin("badgers")
        .unwrap()
        .args(["cov", "--all", "--lcov-file"])
        .arg(lcov)
        .arg("--repo-root")
        .arg(repo.path())
        .args(extra)
        .current_dir(repo.path())
        .assert()
}

#[test]
fn branch_gate_errors_without_branch_data() {
    let repo = init_repo();
    let lcov = repo.path().join("line-only.lcov");
    std::fs::write(&lcov, "SF:lib/a.dart\nDA:1,1\nend_of_record\n").unwrap();

    cov_all(&repo, &lcov, &[]).success();
    cov_all(&repo, &lcov, &["--fail-on-partial-branches"])
        .code(1)
        .stderr(predicate::str::contains("requires branch data"));
}

#[test]
fn branch_gate_fails_on_partial_branches_only_with_flag() {
    let repo = init_repo();
    let lcov = repo.path().join("partial.lcov");
    std::fs::write(
        &lcov,
        "SF:lib/a.dart\nDA:1,1\nBRDA:1,0,0,1\nBRDA:1,0,1,0\nend_of_record\n",
    )
    .unwrap();

    cov_all(&repo, &lcov, &[])
        .success()
        .stdout(predicate::str::contains("lib/a.dart:1 [branch-partial]"));
    cov_all(&repo, &lcov, &["--fail-on-partial-branches"]).code(1);
}

#[test]
fn branch_gate_passes_with_fully_taken_branches() {
    let repo = init_repo();
    let lcov = repo.path().join("full.lcov");
    std::fs::write(
        &lcov,
        "SF:lib/a.dart\nDA:1,2\nBRDA:1,0,0,1\nBRDA:1,0,1,2\nend_of_record\n",
    )
    .unwrap();

    cov_all(&repo, &lcov, &["--fail-on-partial-branches"]).success();
}
