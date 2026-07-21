use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use badge_rs_storage::{BranchPointer, Keys, POINTER_SCHEMA_VERSION};

pub(crate) const DEFAULT_STORAGE_BRANCH: &str = "badgers-coverage";
pub(crate) const DEFAULT_STORAGE_PREFIX: &str = "badgers";
pub(crate) const SHELL_INSTALL_COMMAND: &str = "curl --proto '=https' --tlsv1.2 -LsSf https://github.com/devgony/badgers/releases/latest/download/badgers-installer.sh | sh";

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SourceRepository {
    pub(crate) slug: String,
    pub(crate) origin_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GithubReportLocation {
    pub source_repo: String,
    pub storage_repo: String,
    pub storage_branch: String,
    pub storage_prefix: String,
}

impl GithubReportLocation {
    pub fn new(
        source_repo: &str,
        storage_repo: &str,
        storage_branch: &str,
        storage_prefix: &str,
    ) -> Result<Self> {
        validate_repo_slug(source_repo, "source repository")?;
        validate_repo_slug(storage_repo, "storage repository")?;
        validate_branch(storage_branch)?;
        let storage_prefix = normalize_prefix(storage_prefix)?;
        Ok(Self {
            source_repo: source_repo.to_string(),
            storage_repo: storage_repo.to_string(),
            storage_branch: storage_branch.to_string(),
            storage_prefix,
        })
    }

    pub fn pr_pointer_path(&self, pr: u64) -> String {
        Keys::new(&self.storage_prefix, &self.source_repo).pr_pointer(pr)
    }

    pub fn html_index_path(&self, sha: &str) -> Result<String> {
        Ok(format!("{}/index.html", self.html_prefix(sha)?))
    }

    pub fn comparison_path(&self, sha: &str) -> Result<String> {
        validate_sha(sha)?;
        Ok(Keys::new(&self.storage_prefix, &self.source_repo).commit_comparison(sha))
    }

    pub fn html_prefix(&self, sha: &str) -> Result<String> {
        validate_sha(sha)?;
        Ok(Keys::new(&self.storage_prefix, &self.source_repo).commit_html_prefix(sha))
    }

    pub fn report_spec(&self, sha: &str) -> Result<String> {
        Ok(format!(
            "{}@{}:{}",
            self.storage_repo,
            self.storage_branch,
            self.html_index_path(sha)?
        ))
    }

    pub fn installed_view_command(&self, pr: u64) -> String {
        self.view_command_with_binary(pr, "~/.local/bin/badgers")
    }

    fn view_command_with_binary(&self, pr: u64, binary: &str) -> String {
        let mut command = format!(
            "{binary} view {pr} --repo {} --storage-repo {}",
            shell_quote(&self.source_repo),
            shell_quote(&self.storage_repo)
        );
        if self.storage_branch != DEFAULT_STORAGE_BRANCH {
            command.push_str(" --storage-branch ");
            command.push_str(&shell_quote(&self.storage_branch));
        }
        if self.storage_prefix != DEFAULT_STORAGE_PREFIX {
            command.push_str(" --storage-prefix ");
            command.push_str(&shell_quote(&self.storage_prefix));
        }
        command
    }
}

pub(crate) fn resolve_source_repo(explicit: Option<&str>) -> Result<SourceRepository> {
    let origin_url = local_origin_url();
    let origin_repo = origin_url.as_deref().and_then(parse_repo_url);

    let slug = if let Some(repo) = explicit {
        repo.to_string()
    } else if let Ok(repo) = std::env::var("GITHUB_REPOSITORY")
        && !repo.is_empty()
    {
        repo
    } else if let Some(repo) = &origin_repo {
        repo.clone()
    } else {
        github_cli_repo()?
    };
    validate_repo_slug(&slug, "source repository")?;
    let origin_url = (origin_repo.as_deref() == Some(&slug))
        .then_some(origin_url)
        .flatten();
    Ok(SourceRepository { slug, origin_url })
}

fn github_cli_repo() -> Result<String> {
    let output = Command::new("gh")
        .args([
            "repo",
            "view",
            "--json",
            "nameWithOwner",
            "--jq",
            ".nameWithOwner",
        ])
        .output()
        .context("running gh repo view; install and authenticate GitHub CLI or pass --repo")?;
    if !output.status.success() {
        bail!(
            "gh repo view failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let repo = String::from_utf8(output.stdout)
        .context("gh repo view returned non-UTF-8 output")?
        .trim()
        .to_string();
    validate_repo_slug(&repo, "repository reported by gh")?;
    Ok(repo)
}

fn local_origin_url() -> Option<String> {
    let output = Command::new("git")
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!url.is_empty()).then_some(url)
}

pub(crate) fn clone_storage_branch(
    location: &GithubReportLocation,
    source_origin_url: Option<&str>,
    destination: &Path,
) -> Result<()> {
    let status = if let Some(origin_url) =
        source_origin_url.filter(|_| location.storage_repo == location.source_repo)
    {
        Command::new("git")
            .args([
                "clone",
                "--quiet",
                "--depth",
                "1",
                "--single-branch",
                "--branch",
            ])
            .arg(&location.storage_branch)
            .arg(origin_url)
            .arg(destination)
            .status()
            .context("running git clone for the local origin remote")?
    } else {
        Command::new("gh")
            .args(["repo", "clone", &location.storage_repo])
            .arg(destination)
            .arg("--")
            .args(["--quiet", "--depth", "1", "--single-branch", "--branch"])
            .arg(&location.storage_branch)
            .status()
            .context("running gh repo clone; install and authenticate GitHub CLI first")?
    };
    ensure!(
        status.success(),
        "could not clone {} branch {}",
        location.storage_repo,
        location.storage_branch
    );
    Ok(())
}

pub(crate) fn read_pointer(
    location: &GithubReportLocation,
    pr: u64,
    checkout: &Path,
) -> Result<BranchPointer> {
    let key = location.pr_pointer_path(pr);
    let path = checked_repo_path(checkout, &key)
        .with_context(|| format!("locating PR #{pr} pointer at {key}"))?;
    ensure!(path.is_file(), "PR #{pr} pointer is not a file");
    let data = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let pointer: BranchPointer =
        serde_json::from_slice(&data).with_context(|| format!("parsing {}", path.display()))?;
    ensure!(
        pointer.schema_version == POINTER_SCHEMA_VERSION,
        "unsupported PR pointer schema version {}",
        pointer.schema_version
    );
    ensure!(
        pointer.branch == format!("pr-{pr}"),
        "pointer belongs to {}, not PR #{pr}",
        pointer.branch
    );
    validate_sha(&pointer.commit_sha)?;
    Ok(pointer)
}

pub(crate) fn checked_repo_path(root: &Path, key: &str) -> Result<PathBuf> {
    ensure!(!key.is_empty(), "stored report path is empty");
    let mut path = root.to_path_buf();
    for component in key.split('/') {
        ensure!(
            !component.is_empty() && !matches!(component, "." | "..") && !component.contains('\\'),
            "stored report contains an unsafe path: {key}"
        );
        path.push(component);
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("reading stored report path {}", path.display()))?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "stored report path contains a symlink: {}",
            path.display()
        );
    }
    Ok(path)
}

pub(crate) fn validate_repo_slug(value: &str, label: &str) -> Result<()> {
    let mut parts = value.split('/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    if parts.next().is_some()
        || !valid_repo_part(owner)
        || !valid_repo_part(repo)
        || matches!(owner, "." | "..")
        || matches!(repo, "." | "..")
    {
        bail!("{label} must be owner/repo");
    }
    Ok(())
}

pub(crate) fn parse_repo_url(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches(".git");
    let tail = trimmed
        .rsplit_once(':')
        .map(|(_, tail)| tail)
        .unwrap_or(trimmed)
        .trim_start_matches('/');
    let mut segments = tail.rsplit('/');
    let name = segments.next()?;
    let owner = segments.next()?;
    let repo = format!("{owner}/{name}");
    validate_repo_slug(&repo, "Git remote repository")
        .ok()
        .map(|()| repo)
}

fn valid_repo_part(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn validate_branch(value: &str) -> Result<()> {
    if value.is_empty()
        || value.starts_with('-')
        || value.starts_with('/')
        || value.ends_with('/')
        || value.ends_with('.')
        || value.contains("..")
        || value.contains("//")
        || value.contains("@{")
        || value.split('/').any(|part| part.ends_with(".lock"))
        || value.bytes().any(|byte| {
            byte.is_ascii_control()
                || byte.is_ascii_whitespace()
                || matches!(byte, b'\\' | b'~' | b'^' | b':' | b'?' | b'*' | b'[')
        })
    {
        bail!("storage branch is not a safe Git branch name");
    }
    Ok(())
}

fn normalize_prefix(value: &str) -> Result<String> {
    if value.contains('\\') || value.chars().any(char::is_control) {
        bail!("storage prefix is unsafe");
    }
    let normalized = value.trim_matches('/');
    if normalized
        .split('/')
        .any(|part| part.is_empty() || matches!(part, "." | ".."))
        && !normalized.is_empty()
    {
        bail!("storage prefix is unsafe");
    }
    Ok(normalized.to_string())
}

pub(crate) fn validate_sha(value: &str) -> Result<()> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("commit SHA must be exactly 40 ASCII hexadecimal characters");
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/' | b':' | b'@')
        })
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

pub(crate) fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn builds_pointer_report_path_and_view_command() {
        let location = GithubReportLocation::new(
            "owner/project",
            "reports/archive",
            "coverage/reports",
            "/badgers/history/",
        )
        .unwrap();
        assert_eq!(
            location.pr_pointer_path(547),
            "badgers/history/repos/owner/project/prs/547/latest.json"
        );
        assert_eq!(
            location.report_spec(SHA).unwrap(),
            "reports/archive@coverage/reports:badgers/history/repos/owner/project/commits/0123456789abcdef0123456789abcdef01234567/html/index.html"
        );
        assert_eq!(
            location.html_prefix(SHA).unwrap(),
            "badgers/history/repos/owner/project/commits/0123456789abcdef0123456789abcdef01234567/html"
        );
        assert_eq!(
            location.comparison_path(SHA).unwrap(),
            "badgers/history/repos/owner/project/commits/0123456789abcdef0123456789abcdef01234567/comparison.json.zst"
        );
        assert_eq!(
            location.view_command_with_binary(547, "badgers"),
            "badgers view 547 --repo owner/project --storage-repo reports/archive --storage-branch coverage/reports --storage-prefix badgers/history"
        );
        assert_eq!(
            location.installed_view_command(547),
            "~/.local/bin/badgers view 547 --repo owner/project --storage-repo reports/archive --storage-branch coverage/reports --storage-prefix badgers/history"
        );
    }

    #[test]
    fn omits_default_storage_options_from_view_command() {
        let location = GithubReportLocation::new(
            "owner/project",
            "owner/project",
            DEFAULT_STORAGE_BRANCH,
            DEFAULT_STORAGE_PREFIX,
        )
        .unwrap();
        assert_eq!(
            location.view_command_with_binary(5, "badgers"),
            "badgers view 5 --repo owner/project --storage-repo owner/project"
        );
    }

    #[test]
    fn rejects_unsafe_locations() {
        assert!(GithubReportLocation::new("owner/../repo", "o/r", "main", "badgers").is_err());
        assert!(GithubReportLocation::new("o/r", "o/r", "../main", "badgers").is_err());
        assert!(GithubReportLocation::new("o/r", "o/r", "main", "badgers/../x").is_err());
        assert!(validate_sha("abc123").is_err());
    }

    #[test]
    fn parses_common_git_remote_urls() {
        for url in [
            "git@github.com:owner/repo.git",
            "https://github.com/owner/repo.git",
            "https://github.com/owner/repo",
            "ssh://git@github.com/owner/repo.git",
        ] {
            assert_eq!(parse_repo_url(url).as_deref(), Some("owner/repo"), "{url}");
        }
        assert_eq!(parse_repo_url("not-a-url"), None);
        assert_eq!(parse_repo_url("https://github.com/owner/../repo"), None);
    }

    #[test]
    fn reads_matching_pointer_and_rejects_wrong_pr() {
        let temp = tempfile::tempdir().unwrap();
        let location = GithubReportLocation::new(
            "owner/project",
            "reports/archive",
            DEFAULT_STORAGE_BRANCH,
            DEFAULT_STORAGE_PREFIX,
        )
        .unwrap();
        let pointer_path = temp.path().join(location.pr_pointer_path(547));
        std::fs::create_dir_all(pointer_path.parent().unwrap()).unwrap();
        let pointer = BranchPointer {
            schema_version: POINTER_SCHEMA_VERSION,
            branch: "pr-547".into(),
            commit_sha: SHA.into(),
            committed_at: "2026-07-20T00:00:00Z".into(),
            snapshot_key: "snapshot".into(),
            comparison_key: None,
            report_key: None,
            html_prefix: Some(location.html_prefix(SHA).unwrap()),
            updated_at: "2026-07-20T00:00:00Z".into(),
        };
        std::fs::write(&pointer_path, serde_json::to_vec(&pointer).unwrap()).unwrap();
        assert_eq!(read_pointer(&location, 547, temp.path()).unwrap(), pointer);

        let mut wrong = pointer;
        wrong.branch = "pr-548".into();
        std::fs::write(&pointer_path, serde_json::to_vec(&wrong).unwrap()).unwrap();
        assert!(read_pointer(&location, 547, temp.path()).is_err());
    }

    #[test]
    fn recognizes_when_local_origin_can_clone_storage_directly() {
        let source = SourceRepository {
            slug: "owner/project".into(),
            origin_url: Some("git@github.com:owner/project.git".into()),
        };
        let location = GithubReportLocation::new(
            &source.slug,
            "owner/project",
            DEFAULT_STORAGE_BRANCH,
            DEFAULT_STORAGE_PREFIX,
        )
        .unwrap();
        assert_eq!(location.storage_repo, location.source_repo);
        assert_eq!(
            source.origin_url.as_deref(),
            Some("git@github.com:owner/project.git")
        );
    }
}
