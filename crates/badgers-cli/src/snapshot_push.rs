use anyhow::Context;
use badgers_storage::{
    BranchPointer, POINTER_SCHEMA_VERSION, PointerUpdate, PutOptions, update_pointer_if_newer,
};
use clap::Args;

use crate::storage_opts::StorageOpts;

/// Upload a snapshot to `commits/{sha}/` and advance the branch / PR pointer.
#[derive(Args)]
pub struct SnapshotPushArgs {
    /// Snapshot JSON produced by `badgers collect`
    #[arg(long, value_name = "FILE")]
    pub snapshot: std::path::PathBuf,

    /// Full commit SHA the snapshot was measured at
    #[arg(long)]
    pub sha: String,

    /// Git commit timestamp (RFC 3339), e.g. `git show -s --format=%cI <sha>`
    #[arg(long)]
    pub committed_at: String,

    /// Branch to advance `refs/{branch}/latest.json` for (push events)
    #[arg(long)]
    pub branch: Option<String>,

    /// PR number to advance `prs/{n}/latest.json` for (pull_request events)
    #[arg(long)]
    pub pr: Option<u64>,

    #[command(flatten)]
    pub storage: StorageOpts,
}

const ZSTD_LEVEL: i32 = 3;

pub fn run(args: &SnapshotPushArgs) -> anyhow::Result<()> {
    let backend = args.storage.backend()?;
    let keys = args.storage.keys();

    let json = std::fs::read(&args.snapshot)
        .with_context(|| format!("reading snapshot {}", args.snapshot.display()))?;
    let compressed = zstd::encode_all(json.as_slice(), ZSTD_LEVEL).context("zstd compression")?;

    let snapshot_key = keys.commit_snapshot(&args.sha);
    backend.put(
        &snapshot_key,
        &compressed,
        PutOptions {
            if_generation_match: None,
            content_type: "application/zstd",
        },
    )?;
    println!("uploaded: {snapshot_key} ({} bytes)", compressed.len());

    let mut pointer_keys = Vec::new();
    if let Some(branch) = &args.branch {
        pointer_keys.push((keys.branch_pointer(branch), branch.clone()));
    }
    if let Some(pr) = args.pr {
        pointer_keys.push((keys.pr_pointer(pr), format!("pr-{pr}")));
    }

    let updated_at = jiff::Timestamp::now().to_string();
    for (key, label) in pointer_keys {
        let pointer = BranchPointer {
            schema_version: POINTER_SCHEMA_VERSION,
            branch: label,
            commit_sha: args.sha.clone(),
            committed_at: args.committed_at.clone(),
            snapshot_key: snapshot_key.clone(),
            updated_at: updated_at.clone(),
        };
        match update_pointer_if_newer(backend.as_ref(), &key, &pointer)? {
            PointerUpdate::Updated => println!("pointer updated: {key}"),
            PointerUpdate::SkippedOlder => {
                println!("pointer skipped (already at a newer commit): {key}")
            }
        }
    }
    Ok(())
}
