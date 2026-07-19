use anyhow::Context;
use badge_rs_core::CoverageSnapshot;
use badge_rs_core::compare::{COMPARISON_SCHEMA_VERSION, ComparisonDocument};
use badge_rs_storage::{
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

    /// Comparison JSON to store alongside the snapshot
    #[arg(long, value_name = "FILE")]
    pub comparison: Option<std::path::PathBuf>,

    /// Markdown report to store alongside the snapshot and current pointer
    #[arg(long, value_name = "FILE")]
    pub report: Option<std::path::PathBuf>,

    #[command(flatten)]
    pub storage: StorageOpts,
}

const ZSTD_LEVEL: i32 = 3;

pub fn run(args: &SnapshotPushArgs) -> anyhow::Result<()> {
    let backend = args.storage.backend()?;
    let keys = args.storage.keys();

    let json = std::fs::read(&args.snapshot)
        .with_context(|| format!("reading snapshot {}", args.snapshot.display()))?;
    let snapshot: CoverageSnapshot = serde_json::from_slice(&json)
        .with_context(|| format!("parsing snapshot {}", args.snapshot.display()))?;
    anyhow::ensure!(
        snapshot.commit_sha == args.sha,
        "snapshot commit SHA {} does not match --sha {}",
        snapshot.commit_sha,
        args.sha
    );
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

    let comparison_key = if let Some(path) = &args.comparison {
        let json = std::fs::read(path)
            .with_context(|| format!("reading comparison {}", path.display()))?;
        let comparison: ComparisonDocument = serde_json::from_slice(&json)
            .with_context(|| format!("parsing comparison {}", path.display()))?;
        anyhow::ensure!(
            comparison.schema_version == COMPARISON_SCHEMA_VERSION,
            "unsupported comparison schema version {}",
            comparison.schema_version
        );
        anyhow::ensure!(
            comparison.head_sha == args.sha,
            "comparison head SHA {} does not match --sha {}",
            comparison.head_sha,
            args.sha
        );
        let compressed =
            zstd::encode_all(json.as_slice(), ZSTD_LEVEL).context("zstd comparison compression")?;
        let key = keys.commit_comparison(&args.sha);
        backend.put(
            &key,
            &compressed,
            PutOptions {
                if_generation_match: None,
                content_type: "application/zstd",
            },
        )?;
        println!("uploaded: {key} ({} bytes)", compressed.len());
        Some(key)
    } else {
        None
    };

    let report = args
        .report
        .as_ref()
        .map(|path| {
            std::fs::read(path).with_context(|| format!("reading report {}", path.display()))
        })
        .transpose()?;
    let report_key = report.as_ref().map(|_| keys.commit_report(&args.sha));
    if let (Some(markdown), Some(key)) = (&report, &report_key) {
        put_markdown(backend.as_ref(), key, markdown)?;
    }
    let materialize_report_alias = args.storage.local_dir.is_some();

    let mut pointer_keys = Vec::new();
    if let Some(branch) = &args.branch {
        pointer_keys.push((
            keys.branch_pointer(branch),
            keys.branch_report(branch),
            branch.clone(),
        ));
    }
    if let Some(pr) = args.pr {
        pointer_keys.push((keys.pr_pointer(pr), keys.pr_report(pr), format!("pr-{pr}")));
    }

    let updated_at = jiff::Timestamp::now().to_string();
    for (key, report_alias_key, label) in pointer_keys {
        let pointer = BranchPointer {
            schema_version: POINTER_SCHEMA_VERSION,
            branch: label,
            commit_sha: args.sha.clone(),
            committed_at: args.committed_at.clone(),
            snapshot_key: snapshot_key.clone(),
            comparison_key: comparison_key.clone(),
            report_key: report_key.clone(),
            updated_at: updated_at.clone(),
        };
        match update_pointer_if_newer(backend.as_ref(), &key, &pointer)? {
            PointerUpdate::Updated => {
                println!("pointer updated: {key}");
                if materialize_report_alias && let Some(markdown) = &report {
                    put_markdown(backend.as_ref(), &report_alias_key, markdown)?;
                }
            }
            PointerUpdate::SkippedOlder => {
                println!("pointer skipped (already at a newer commit): {key}")
            }
        }
    }
    Ok(())
}

fn put_markdown(
    backend: &dyn badge_rs_storage::StorageBackend,
    key: &str,
    markdown: &[u8],
) -> anyhow::Result<()> {
    backend.put(
        key,
        markdown,
        PutOptions {
            if_generation_match: None,
            content_type: "text/markdown; charset=utf-8",
        },
    )?;
    println!("uploaded: {key} ({} bytes)", markdown.len());
    Ok(())
}
