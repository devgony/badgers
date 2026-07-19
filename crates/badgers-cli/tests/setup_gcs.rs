use assert_cmd::Command;
use predicates::prelude::*;

const FAKE_GCLOUD: &str = r#"#!/bin/bash
echo "gcloud $*" >> "$FAKE_LOG"
case "$*" in
  "projects describe my-proj --format=value(projectNumber)")
    echo "123456789012" ;;
  *describe*)
    if [ "${FAKE_EXISTS:-0}" = "1" ]; then
      case "$*" in
        *providers*attributeCondition*) echo "assertion.repository_id == '987'" ;;
        *) echo "exists" ;;
      esac
    else
      echo "not found" >&2; exit 1
    fi ;;
  *) exit 0 ;;
esac
"#;

const FAKE_GH: &str = r#"#!/bin/bash
echo "gh $*" >> "$FAKE_LOG"
echo "987"
"#;

struct Fixture {
    _dir: tempfile::TempDir,
    bin_dir: std::path::PathBuf,
    log: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir(&bin_dir).unwrap();
        for (name, body) in [("gcloud", FAKE_GCLOUD), ("gh", FAKE_GH)] {
            let path = bin_dir.join(name);
            std::fs::write(&path, body).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let log = dir.path().join("calls.log");
        std::fs::write(&log, "").unwrap();
        Self {
            bin_dir,
            log,
            _dir: dir,
        }
    }

    fn cmd(&self, exists: bool) -> Command {
        let mut cmd = Command::cargo_bin("badgers").unwrap();
        let path = format!(
            "{}:{}",
            self.bin_dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        cmd.env("PATH", path)
            .env("FAKE_LOG", &self.log)
            .env("FAKE_EXISTS", if exists { "1" } else { "0" })
            .args([
                "setup",
                "gcs",
                "--project",
                "my-proj",
                "--repo",
                "jubilee-works/timetree-planner-agent",
            ]);
        cmd
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap()
    }
}

#[test]
fn fresh_project_creates_all_resources_and_prints_snippet() {
    let fx = Fixture::new();
    fx.cmd(false)
        .assert()
        .success()
        .stdout(
            predicate::str::contains("workload_identity_provider: projects/123456789012/locations/global/workloadIdentityPools/github-actions/providers/gh-timetree-planner-agent")
                .and(predicate::str::contains(
                    "service_account: badgers-timetree-planner-agent@my-proj.iam.gserviceaccount.com",
                ))
                .and(predicate::str::contains("gcs-bucket: my-proj-badgers-coverage")),
        );

    let calls = fx.calls();
    for needle in [
        "services enable iam.googleapis.com",
        "storage buckets create gs://my-proj-badgers-coverage --project my-proj --location asia-northeast3 --uniform-bucket-level-access --public-access-prevention",
        "workload-identity-pools create github-actions",
        "providers create-oidc gh-timetree-planner-agent",
        "--attribute-condition=assertion.repository_id == '987'",
        "service-accounts create badgers-timetree-planner-agent",
        "--member=principalSet://iam.googleapis.com/projects/123456789012/locations/global/workloadIdentityPools/github-actions/attribute.repository_id/987",
        "buckets add-iam-policy-binding gs://my-proj-badgers-coverage --member=serviceAccount:badgers-timetree-planner-agent@my-proj.iam.gserviceaccount.com --role=roles/storage.objectUser",
    ] {
        assert!(
            calls.contains(needle),
            "missing call: {needle}\n---\n{calls}"
        );
    }
}

#[test]
fn existing_resources_are_skipped_but_bindings_still_applied() {
    let fx = Fixture::new();
    fx.cmd(true).assert().success().stdout(
        predicate::str::contains("bucket already exists")
            .and(predicate::str::contains("pool already exists"))
            .and(predicate::str::contains("provider already exists"))
            .and(predicate::str::contains("service account already exists")),
    );

    let calls = fx.calls();
    assert!(!calls.contains("buckets create"), "{calls}");
    assert!(!calls.contains("create-oidc"), "{calls}");
    assert!(calls.contains("add-iam-policy-binding"), "{calls}");
}

#[test]
fn dry_run_skips_mutations() {
    let fx = Fixture::new();
    fx.cmd(false)
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(predicate::str::contains("[dry-run] create private bucket"));

    let calls = fx.calls();
    assert!(!calls.contains("buckets create"), "{calls}");
    assert!(!calls.contains("services enable"), "{calls}");
    assert!(calls.contains("projects describe"), "{calls}");
}
