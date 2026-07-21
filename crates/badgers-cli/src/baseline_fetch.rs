use anyhow::Context;
use badge_rs_storage::BranchPointer;
use clap::Args;

use crate::storage_opts::StorageOpts;

/// Resolve and download the baseline snapshot: exact merge-base snapshot
/// first, then the base branch pointer (approximate), otherwise none.
#[derive(Args)]
pub struct BaselineFetchArgs {
    /// Merge-base commit SHA between the PR head and the base branch
    #[arg(long)]
    pub merge_base: String,

    /// Base branch name, e.g. "main"
    #[arg(long)]
    pub base_ref: String,

    /// Where to write the decompressed baseline snapshot JSON
    #[arg(short, long, value_name = "FILE")]
    pub output: std::path::PathBuf,

    #[command(flatten)]
    pub storage: StorageOpts,
}

pub fn run(args: &BaselineFetchArgs) -> anyhow::Result<()> {
    validate_commit_sha(&args.merge_base).context("invalid merge-base commit SHA")?;
    let backend = args.storage.backend()?;
    let keys = args.storage.keys();

    let exact_key = keys.commit_snapshot(&args.merge_base);
    if let Some(obj) = backend.get(&exact_key)? {
        write_snapshot(&args.output, &obj.data)?;
        report("exact", &args.merge_base);
        return Ok(());
    }

    let pointer_key = keys.branch_pointer(&args.base_ref);
    if let Some(obj) = backend.get(&pointer_key)? {
        let pointer: BranchPointer =
            serde_json::from_slice(&obj.data).with_context(|| format!("parsing {pointer_key}"))?;
        validate_commit_sha(&pointer.commit_sha)
            .with_context(|| format!("invalid commit SHA in {pointer_key}"))?;
        let snapshot = backend.get(&pointer.snapshot_key)?.with_context(|| {
            format!(
                "pointer {pointer_key} references missing object {}",
                pointer.snapshot_key
            )
        })?;
        write_snapshot(&args.output, &snapshot.data)?;
        report("approximate", &pointer.commit_sha);
        return Ok(());
    }

    report("none", "");
    Ok(())
}

fn write_snapshot(output: &std::path::Path, compressed: &[u8]) -> anyhow::Result<()> {
    let json = zstd::decode_all(compressed).context("zstd decompression")?;
    std::fs::write(output, json).with_context(|| format!("writing {}", output.display()))
}

/// Prints `key=value` lines so the action step can pipe stdout straight into
/// `$GITHUB_OUTPUT`.
fn report(kind: &str, sha: &str) {
    println!("baseline-kind={kind}");
    println!("baseline-sha={sha}");
}

fn validate_commit_sha(sha: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "commit SHA must be exactly 40 ASCII hexadecimal characters"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_pointer_commit_sha_before_reporting() {
        assert!(validate_commit_sha("0123456789abcdef0123456789abcdef01234567").is_ok());
        for invalid in [
            "abc1234",
            "0123456789abcdef0123456789abcdef0123456g",
            "0123456789abcdef0123456789abcdef012345\n",
        ] {
            assert!(
                validate_commit_sha(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
    }
}
