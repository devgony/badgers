use anyhow::{Result, bail};
use badge_rs_storage::Keys;

pub(crate) const DEFAULT_STORAGE_BRANCH: &str = "badgers-coverage";
pub(crate) const DEFAULT_STORAGE_PREFIX: &str = "badgers";

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

    pub fn view_command(&self, pr: u64) -> String {
        let mut command = format!(
            "badgers view {pr} --repo {} --storage-repo {}",
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
            location.view_command(547),
            "badgers view 547 --repo owner/project --storage-repo reports/archive --storage-branch coverage/reports --storage-prefix badgers/history"
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
            location.view_command(5),
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
}
