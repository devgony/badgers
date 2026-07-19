use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

/// Everything except `[A-Za-z0-9._-]` is escaped, so encoding is reversible
/// and collision-free (unlike `__` substitution, where `a/b` and `a__b` clash).
const BRANCH_ESCAPE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'.')
    .remove(b'_')
    .remove(b'-');

pub fn encode_branch(branch: &str) -> String {
    utf8_percent_encode(branch, BRANCH_ESCAPE).to_string()
}

/// Object key builder for the badgers storage layout:
///
/// ```text
/// {prefix}/repos/{owner}/{repo}/
///   commits/{sha}/coverage.json.zst
///   refs/{encoded_branch}/latest.json
///   prs/{pr_number}/latest.json
/// ```
#[derive(Debug, Clone)]
pub struct Keys {
    root: String,
}

impl Keys {
    pub fn new(prefix: &str, repo: &str) -> Self {
        let prefix = prefix.trim_matches('/');
        let repo = repo.trim_matches('/');
        let root = if prefix.is_empty() {
            format!("repos/{repo}")
        } else {
            format!("{prefix}/repos/{repo}")
        };
        Self { root }
    }

    pub fn commit_snapshot(&self, sha: &str) -> String {
        format!("{}/commits/{sha}/coverage.json.zst", self.root)
    }

    pub fn branch_pointer(&self, branch: &str) -> String {
        format!("{}/refs/{}/latest.json", self.root, encode_branch(branch))
    }

    pub fn pr_pointer(&self, pr_number: u64) -> String {
        format!("{}/prs/{pr_number}/latest.json", self.root)
    }

    pub fn pr_report_dir(&self, pr_number: u64) -> String {
        format!("{}/prs/{pr_number}/report", self.root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_branch_escapes_slash_reversibly() {
        assert_eq!(encode_branch("main"), "main");
        assert_eq!(encode_branch("release/1.2"), "release%2F1.2");
        assert_eq!(encode_branch("feat_x-y.z"), "feat_x-y.z");
        assert_eq!(encode_branch("한글"), "%ED%95%9C%EA%B8%80");
    }

    #[test]
    fn keys_layout() {
        let keys = Keys::new("badgers", "owner/repo");
        assert_eq!(
            keys.commit_snapshot("abc123"),
            "badgers/repos/owner/repo/commits/abc123/coverage.json.zst"
        );
        assert_eq!(
            keys.branch_pointer("release/1.2"),
            "badgers/repos/owner/repo/refs/release%2F1.2/latest.json"
        );
        assert_eq!(
            keys.pr_pointer(547),
            "badgers/repos/owner/repo/prs/547/latest.json"
        );
        assert_eq!(
            keys.pr_report_dir(547),
            "badgers/repos/owner/repo/prs/547/report"
        );
    }

    #[test]
    fn keys_empty_prefix() {
        let keys = Keys::new("", "owner/repo");
        assert_eq!(
            keys.commit_snapshot("abc"),
            "repos/owner/repo/commits/abc/coverage.json.zst"
        );
    }
}
