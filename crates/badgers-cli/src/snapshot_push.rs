use std::path::Path;

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

    /// HTML report directory: every file is stored under `commits/{sha}/html/`
    /// and `html_prefix` is recorded in the pointer
    #[arg(long, value_name = "DIR")]
    pub html_report: Option<std::path::PathBuf>,

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

    let html_prefix = if let Some(dir) = &args.html_report {
        let dir_meta = std::fs::symlink_metadata(dir)
            .with_context(|| format!("stat --html-report {}", dir.display()))?;
        anyhow::ensure!(
            !dir_meta.file_type().is_symlink(),
            "--html-report path is a symlink: {}",
            dir.display()
        );
        anyhow::ensure!(
            dir_meta.is_dir(),
            "--html-report is not a directory: {}",
            dir.display()
        );
        let files = walk_html_report(dir, dir)?;
        let prefix = keys.commit_html_prefix(&args.sha);
        for (relative, data) in &files {
            let key = format!("{prefix}/{relative}");
            backend.put(
                &key,
                data,
                PutOptions {
                    if_generation_match: None,
                    content_type: html_content_type(relative),
                },
            )?;
            println!("uploaded: {key} ({} bytes)", data.len());
        }
        Some(prefix)
    } else {
        None
    };

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
            html_prefix: html_prefix.clone(),
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

fn validate_html_path_component(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('\\') {
        anyhow::bail!("unsafe HTML report filename component: {:?}", name);
    }
    Ok(())
}

fn walk_html_report(dir: &Path, base: &Path) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    let mut results = Vec::new();
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("reading html report directory {}", dir.display()))?
    {
        let entry = entry.with_context(|| format!("iterating {}", dir.display()))?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        validate_html_path_component(&name_str)?;
        let path = entry.path();
        let meta =
            std::fs::symlink_metadata(&path).with_context(|| format!("stat {}", path.display()))?;
        if meta.file_type().is_symlink() {
            anyhow::bail!("HTML report contains a symlink: {}", path.display());
        }
        if meta.is_dir() {
            for item in walk_html_report(&path, base)? {
                results.push(item);
            }
        } else {
            let relative = path.strip_prefix(base).expect("path is under base");
            let relative_key = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let data =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            results.push((relative_key, data));
        }
    }
    Ok(results)
}

fn html_content_type(filename: &str) -> &'static str {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "ico" => "image/x-icon",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}
