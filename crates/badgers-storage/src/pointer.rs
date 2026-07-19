use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{PutOptions, StorageBackend, StorageError};

pub const POINTER_SCHEMA_VERSION: u32 = 1;

/// `refs/{encoded_branch}/latest.json` payload: the latest snapshot known for
/// a branch (also reused for `prs/{n}/latest.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchPointer {
    pub schema_version: u32,
    pub branch: String,
    pub commit_sha: String,
    /// Git commit timestamp (RFC 3339). Orders pointer updates so a re-run of
    /// an old workflow cannot roll the pointer back to an older commit.
    pub committed_at: String,
    pub snapshot_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_key: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerUpdate {
    Updated,
    SkippedOlder,
}

const MAX_ATTEMPTS: u32 = 3;

/// Read-compare-conditionally-write loop from the design doc (§5.1):
/// fetch the current pointer with its generation, skip if it already points at
/// a commit at least as new, otherwise write with `ifGenerationMatch` and
/// retry from the read on 412 (another run raced us).
pub fn update_pointer_if_newer(
    backend: &dyn StorageBackend,
    key: &str,
    new: &BranchPointer,
) -> Result<PointerUpdate, StorageError> {
    let new_committed = parse_ts(key, &new.committed_at)?;
    let body = serde_json::to_vec_pretty(new).expect("pointer serializes");

    let mut last_err = None;
    for attempt in 0..MAX_ATTEMPTS {
        let generation = match backend.get(key)? {
            Some(obj) => {
                if let Ok(existing) = serde_json::from_slice::<BranchPointer>(&obj.data)
                    && let Ok(existing_committed) = parse_ts(key, &existing.committed_at)
                    && existing_committed >= new_committed
                {
                    return Ok(PointerUpdate::SkippedOlder);
                }
                obj.generation
            }
            None => 0,
        };

        let opts = PutOptions {
            if_generation_match: Some(generation),
            content_type: "application/json",
        };
        match backend.put(key, &body, opts) {
            Ok(()) => return Ok(PointerUpdate::Updated),
            Err(StorageError::PreconditionFailed { .. }) if attempt + 1 < MAX_ATTEMPTS => {
                last_err = Some(StorageError::PreconditionFailed {
                    key: key.to_string(),
                });
                backoff_sleep(attempt);
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.expect("loop exits early unless a precondition failure was recorded"))
}

fn parse_ts(key: &str, value: &str) -> Result<jiff::Timestamp, StorageError> {
    jiff::Timestamp::from_str(value).map_err(|e| StorageError::Http {
        key: key.to_string(),
        message: format!("invalid committed_at {value:?}: {e}"),
    })
}

fn backoff_sleep(attempt: u32) {
    let jitter_ms = u64::from(std::process::id()) % 100;
    let ms = 200u64 * 2u64.pow(attempt) + jitter_ms;
    std::thread::sleep(std::time::Duration::from_millis(ms));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LocalBackend;

    fn pointer(sha: &str, committed_at: &str) -> BranchPointer {
        BranchPointer {
            schema_version: POINTER_SCHEMA_VERSION,
            branch: "main".to_string(),
            commit_sha: sha.to_string(),
            committed_at: committed_at.to_string(),
            snapshot_key: format!("badgers/repos/o/r/commits/{sha}/coverage.json.zst"),
            comparison_key: None,
            report_key: None,
            updated_at: "2026-07-19T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn writes_when_absent_and_skips_older() {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path());
        let key = "badgers/repos/o/r/refs/main/latest.json";

        let newer = pointer("bbb", "2026-07-19T12:00:00+09:00");
        assert_eq!(
            update_pointer_if_newer(&backend, key, &newer).unwrap(),
            PointerUpdate::Updated
        );

        let older = pointer("aaa", "2026-07-19T02:00:00Z");
        assert_eq!(
            update_pointer_if_newer(&backend, key, &older).unwrap(),
            PointerUpdate::SkippedOlder
        );

        let stored: BranchPointer =
            serde_json::from_slice(&backend.get(key).unwrap().unwrap().data).unwrap();
        assert_eq!(stored.commit_sha, "bbb");
    }

    #[test]
    fn equal_timestamp_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path());
        let key = "badgers/repos/o/r/refs/main/latest.json";

        let first = pointer("aaa", "2026-07-19T03:00:00Z");
        update_pointer_if_newer(&backend, key, &first).unwrap();
        let same_time = pointer("ccc", "2026-07-19T12:00:00+09:00");
        assert_eq!(
            update_pointer_if_newer(&backend, key, &same_time).unwrap(),
            PointerUpdate::SkippedOlder
        );
    }

    #[test]
    fn rejects_unparseable_committed_at() {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path());
        let bad = pointer("aaa", "yesterday");
        assert!(update_pointer_if_newer(&backend, "k/latest.json", &bad).is_err());
    }

    #[test]
    fn reads_legacy_pointer_without_optional_artifact_keys() {
        let json = br#"{
          "schema_version": 1,
          "branch": "main",
          "commit_sha": "abc",
          "committed_at": "2026-07-19T00:00:00Z",
          "snapshot_key": "commits/abc/coverage.json.zst",
          "updated_at": "2026-07-19T00:01:00Z"
        }"#;
        let pointer: BranchPointer = serde_json::from_slice(json).unwrap();
        assert_eq!(pointer.comparison_key, None);
        assert_eq!(pointer.report_key, None);
    }
}
