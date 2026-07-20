//! Minimal GitHub REST client for badgers PR comments and check annotations.

use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum GithubError {
    #[error("github http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("github api returned {status}: {body}")]
    UnexpectedStatus { status: u16, body: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentAction {
    Created,
    Updated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckAnnotation {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub annotation_level: &'static str,
    pub message: String,
    pub title: String,
}

impl CheckAnnotation {
    pub fn warning(path: impl Into<String>, start_line: u32, end_line: u32) -> Self {
        let (lines, verb) = if start_line == end_line {
            (format!("line {start_line}"), "is")
        } else {
            (format!("lines {start_line}-{end_line}"), "are")
        };
        Self {
            path: path.into(),
            start_line,
            end_line,
            annotation_level: "warning",
            message: format!("Changed executable {lines} {verb} not covered."),
            title: "Uncovered changed lines".to_string(),
        }
    }
}

pub struct GithubClient {
    client: reqwest::blocking::Client,
    base_url: String,
    repo: String,
    token: String,
}

#[derive(Deserialize)]
struct IssueComment {
    id: u64,
    body: Option<String>,
    user: Option<IssueCommentUser>,
    performed_via_github_app: Option<IssueCommentApp>,
}

#[derive(Deserialize)]
struct IssueCommentUser {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct IssueCommentApp {
    slug: String,
}

#[derive(Default)]
struct CommentMatches {
    canonical_ids: Vec<u64>,
    legacy_ids: Vec<u64>,
}

#[derive(Deserialize)]
struct CheckRun {
    id: u64,
}

#[derive(Deserialize)]
struct PullRequest {
    head: PullRequestHead,
}

#[derive(Deserialize)]
struct PullRequestHead {
    sha: String,
}

const PER_PAGE: u32 = 100;
const MAX_PAGES: u32 = 10;
const ANNOTATIONS_PER_REQUEST: usize = 50;

impl GithubClient {
    pub fn new(repo: impl Into<String>, token: impl Into<String>) -> Self {
        Self::with_base_url(repo, token, "https://api.github.com")
    }

    pub fn with_base_url(
        repo: impl Into<String>,
        token: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            client: reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("static GitHub HTTP client configuration is valid"),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            repo: repo.into(),
            token: token.into(),
        }
    }

    /// Update the PR comment containing `marker` if one exists, otherwise
    /// create it (marker-based upsert per design doc §8.1).
    pub fn upsert_pr_comment(
        &self,
        pr_number: u64,
        marker: &str,
        body: &str,
    ) -> Result<CommentAction, GithubError> {
        self.upsert_pr_comment_with_aliases(pr_number, marker, &[], body)
    }

    pub fn upsert_pr_comment_with_aliases(
        &self,
        pr_number: u64,
        marker: &str,
        legacy_marker_prefixes: &[&str],
        body: &str,
    ) -> Result<CommentAction, GithubError> {
        let matches = self.find_comments(pr_number, marker, legacy_marker_prefixes)?;
        let comment_id = matches
            .canonical_ids
            .iter()
            .max()
            .or_else(|| matches.legacy_ids.iter().max())
            .copied();
        let action = if let Some(comment_id) = comment_id {
            let url = format!(
                "{}/repos/{}/issues/comments/{comment_id}",
                self.base_url, self.repo
            );
            let resp = self
                .request(self.client.patch(&url))
                .json(&serde_json::json!({ "body": body }))
                .send()?;
            expect_success(resp)?;
            CommentAction::Updated
        } else {
            let url = format!(
                "{}/repos/{}/issues/{pr_number}/comments",
                self.base_url, self.repo
            );
            let resp = self
                .request(self.client.post(&url))
                .json(&serde_json::json!({ "body": body }))
                .send()?;
            expect_success(resp)?;
            CommentAction::Created
        };

        for duplicate_id in matches
            .canonical_ids
            .iter()
            .chain(&matches.legacy_ids)
            .copied()
            .filter(|id| Some(*id) != comment_id)
        {
            let url = format!(
                "{}/repos/{}/issues/comments/{duplicate_id}",
                self.base_url, self.repo
            );
            let response = self.request(self.client.delete(&url)).send()?;
            expect_success(response)?;
        }
        Ok(action)
    }

    pub fn pull_request_head_sha(&self, pr_number: u64) -> Result<String, GithubError> {
        let url = format!("{}/repos/{}/pulls/{pr_number}", self.base_url, self.repo);
        let response = self.request(self.client.get(&url)).send()?;
        let pull_request: PullRequest = expect_success(response)?.json()?;
        Ok(pull_request.head.sha)
    }

    pub fn create_check_run(
        &self,
        name: &str,
        head_sha: &str,
        title: &str,
        summary: &str,
        annotations: &[CheckAnnotation],
    ) -> Result<u64, GithubError> {
        let first_batch = annotations
            .iter()
            .take(ANNOTATIONS_PER_REQUEST)
            .collect::<Vec<_>>();
        let url = format!("{}/repos/{}/check-runs", self.base_url, self.repo);
        let resp = self
            .request(self.client.post(&url))
            .json(&serde_json::json!({
                "name": name,
                "head_sha": head_sha,
                "status": "completed",
                "conclusion": "success",
                "output": {
                    "title": title,
                    "summary": summary,
                    "annotations": first_batch,
                },
            }))
            .send()?;
        let run: CheckRun = expect_success(resp)?.json()?;

        for batch in annotations[annotations.len().min(ANNOTATIONS_PER_REQUEST)..]
            .chunks(ANNOTATIONS_PER_REQUEST)
        {
            let url = format!(
                "{}/repos/{}/check-runs/{}",
                self.base_url, self.repo, run.id
            );
            let resp = self
                .request(self.client.patch(&url))
                .json(&serde_json::json!({
                    "output": {
                        "title": title,
                        "summary": summary,
                        "annotations": batch,
                    },
                }))
                .send()?;
            expect_success(resp)?;
        }
        Ok(run.id)
    }

    fn find_comments(
        &self,
        pr_number: u64,
        marker: &str,
        legacy_marker_prefixes: &[&str],
    ) -> Result<CommentMatches, GithubError> {
        let mut matches = CommentMatches::default();
        for page in 1..=MAX_PAGES {
            let url = format!(
                "{}/repos/{}/issues/{pr_number}/comments?per_page={PER_PAGE}&page={page}",
                self.base_url, self.repo
            );
            let resp = self.request(self.client.get(&url)).send()?;
            let resp = expect_success(resp)?;
            let comments: Vec<IssueComment> = resp.json()?;
            let count = comments.len();
            for comment in comments {
                if !comment.is_owned_by_github_actions() {
                    continue;
                }
                let Some(first_line) = comment.body.as_deref().and_then(|body| body.lines().next())
                else {
                    continue;
                };
                if first_line == marker {
                    matches.canonical_ids.push(comment.id);
                } else if legacy_marker_prefixes
                    .iter()
                    .any(|prefix| valid_legacy_marker(first_line, prefix))
                {
                    matches.legacy_ids.push(comment.id);
                }
            }
            if count < PER_PAGE as usize {
                break;
            }
        }
        Ok(matches)
    }

    fn request(
        &self,
        builder: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        builder
            .bearer_auth(&self.token)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .header("user-agent", "badgers")
    }
}

impl IssueComment {
    fn is_owned_by_github_actions(&self) -> bool {
        self.user.as_ref().is_some_and(|user| user.kind == "Bot")
            && self
                .performed_via_github_app
                .as_ref()
                .is_some_and(|app| app.slug == "github-actions")
    }
}

fn valid_legacy_marker(line: &str, prefix: &str) -> bool {
    line.strip_prefix(prefix)
        .and_then(|suffix| suffix.strip_suffix(" -->"))
        .is_some_and(|sha| sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn expect_success(
    resp: reqwest::blocking::Response,
) -> Result<reqwest::blocking::Response, GithubError> {
    let status = resp.status();
    if status.is_success() {
        Ok(resp)
    } else {
        Err(GithubError::UnexpectedStatus {
            status: status.as_u16(),
            body: resp.text().unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use httpmock::prelude::*;

    use super::*;

    fn client(server: &MockServer) -> GithubClient {
        GithubClient::with_base_url("owner/repo", "tkn", server.base_url())
    }

    #[test]
    fn creates_comment_when_marker_absent() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/owner/repo/issues/5/comments")
                .header("authorization", "Bearer tkn");
            then.status(200).json_body(serde_json::json!([
                { "id": 1, "body": "unrelated" }
            ]));
        });
        let post = server.mock(|when, then| {
            when.method(POST)
                .path("/repos/owner/repo/issues/5/comments")
                .body_contains("badgers-report");
            then.status(201).json_body(serde_json::json!({ "id": 2 }));
        });

        let action = client(&server)
            .upsert_pr_comment(
                5,
                "<!-- badgers-report:owner/repo:5 -->",
                "<!-- badgers-report:owner/repo:5 -->\nhello",
            )
            .unwrap();
        assert_eq!(action, CommentAction::Created);
        post.assert();
    }

    #[test]
    fn updates_existing_marked_comment() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/repos/owner/repo/issues/5/comments");
            then.status(200).json_body(serde_json::json!([{
                "id": 7,
                "body": "<!-- badgers-report:owner/repo:5 -->\nold",
                "user": { "type": "Bot" },
                "performed_via_github_app": { "slug": "github-actions" }
            }]));
        });
        let patch = server.mock(|when, then| {
            when.method(httpmock::Method::PATCH)
                .path("/repos/owner/repo/issues/comments/7")
                .body_contains("new body");
            then.status(200).json_body(serde_json::json!({ "id": 7 }));
        });

        let action = client(&server)
            .upsert_pr_comment(5, "<!-- badgers-report:owner/repo:5 -->", "new body")
            .unwrap();
        assert_eq!(action, CommentAction::Updated);
        patch.assert();
    }

    #[test]
    fn surfaces_permission_errors_with_status() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/repos/owner/repo/issues/5/comments");
            then.status(403)
                .body("Resource not accessible by integration");
        });

        let err = client(&server).upsert_pr_comment(5, "m", "b").unwrap_err();
        assert!(matches!(
            err,
            GithubError::UnexpectedStatus { status: 403, .. }
        ));
    }

    #[test]
    fn updates_legacy_per_head_comment_during_migration() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/repos/owner/repo/issues/5/comments");
            then.status(200).json_body(serde_json::json!([{
                "id": 8,
                "body": "<!-- badgers-report:owner/repo:5:0123456789abcdef0123456789abcdef01234567 -->\nold",
                "user": { "type": "Bot" },
                "performed_via_github_app": { "slug": "github-actions" }
            }]));
        });
        let patch = server.mock(|when, then| {
            when.method(httpmock::Method::PATCH)
                .path("/repos/owner/repo/issues/comments/8")
                .body_contains("badgers-report:owner/repo:5 --");
            then.status(200).json_body(serde_json::json!({ "id": 8 }));
        });

        let action = client(&server)
            .upsert_pr_comment_with_aliases(
                5,
                "<!-- badgers-report:owner/repo:5 -->",
                &["<!-- badgers-report:owner/repo:5:"],
                "<!-- badgers-report:owner/repo:5 -->\nnew",
            )
            .unwrap();
        assert_eq!(action, CommentAction::Updated);
        patch.assert();
    }

    #[test]
    fn prefers_canonical_comment_and_removes_owned_legacy_duplicates() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/repos/owner/repo/issues/5/comments");
            then.status(200).json_body(serde_json::json!([
                {
                    "id": 7,
                    "body": "<!-- badgers-report:owner/repo:5:0123456789abcdef0123456789abcdef01234567 -->\nlegacy",
                    "user": { "type": "Bot" },
                    "performed_via_github_app": { "slug": "github-actions" }
                },
                {
                    "id": 8,
                    "body": "<!-- badgers-report:owner/repo:5 -->\ncanonical",
                    "user": { "type": "Bot" },
                    "performed_via_github_app": { "slug": "github-actions" }
                },
                {
                    "id": 9,
                    "body": "<!-- badgers-report:owner/repo:5:not-a-sha -->\nspoof",
                    "user": { "type": "Bot" },
                    "performed_via_github_app": { "slug": "github-actions" }
                },
                {
                    "id": 10,
                    "body": "<!-- badgers-report:owner/repo:5 -->\nuser spoof",
                    "user": { "type": "User" },
                    "performed_via_github_app": null
                }
            ]));
        });
        let patch = server.mock(|when, then| {
            when.method(httpmock::Method::PATCH)
                .path("/repos/owner/repo/issues/comments/8");
            then.status(200).json_body(serde_json::json!({ "id": 8 }));
        });
        let delete = server.mock(|when, then| {
            when.method(DELETE)
                .path("/repos/owner/repo/issues/comments/7");
            then.status(204);
        });

        let action = client(&server)
            .upsert_pr_comment_with_aliases(
                5,
                "<!-- badgers-report:owner/repo:5 -->",
                &["<!-- badgers-report:owner/repo:5:"],
                "<!-- badgers-report:owner/repo:5 -->\nnew",
            )
            .unwrap();
        assert_eq!(action, CommentAction::Updated);
        patch.assert();
        delete.assert();
    }

    #[test]
    fn reads_current_pull_request_head_sha() {
        let server = MockServer::start();
        let get = server.mock(|when, then| {
            when.method(GET)
                .path("/repos/owner/repo/pulls/5")
                .header("authorization", "Bearer tkn");
            then.status(200).json_body(serde_json::json!({
                "head": { "sha": "abcdef0123456789" }
            }));
        });

        assert_eq!(
            client(&server).pull_request_head_sha(5).unwrap(),
            "abcdef0123456789"
        );
        get.assert();
    }

    #[test]
    fn creates_check_and_appends_annotations_in_batches_of_fifty() {
        let server = MockServer::start();
        let annotations = (1..=51)
            .map(|line| CheckAnnotation::warning(format!("src/file-{line}.rs"), line, line))
            .collect::<Vec<_>>();
        let first_batch = annotations.iter().take(50).collect::<Vec<_>>();
        let create = server.mock(|when, then| {
            when.method(POST)
                .path("/repos/owner/repo/check-runs")
                .json_body(serde_json::json!({
                    "name": "Badgers diff coverage",
                    "head_sha": "abc123",
                    "status": "completed",
                    "conclusion": "success",
                    "output": {
                        "title": "Coverage needs attention",
                        "summary": "51 ranges",
                        "annotations": first_batch,
                    },
                }));
            then.status(201).json_body(serde_json::json!({ "id": 99 }));
        });
        let update = server.mock(|when, then| {
            when.method(httpmock::Method::PATCH)
                .path("/repos/owner/repo/check-runs/99")
                .json_body(serde_json::json!({
                    "output": {
                        "title": "Coverage needs attention",
                        "summary": "51 ranges",
                        "annotations": [&annotations[50]],
                    },
                }));
            then.status(200).json_body(serde_json::json!({ "id": 99 }));
        });

        let id = client(&server)
            .create_check_run(
                "Badgers diff coverage",
                "abc123",
                "Coverage needs attention",
                "51 ranges",
                &annotations,
            )
            .unwrap();
        assert_eq!(id, 99);
        create.assert();
        update.assert();
    }

    #[test]
    fn creates_successful_check_without_annotations() {
        let server = MockServer::start();
        let create = server.mock(|when, then| {
            when.method(POST)
                .path("/repos/owner/repo/check-runs")
                .body_contains(r#""conclusion":"success""#)
                .body_contains(r#""annotations":[]"#);
            then.status(201).json_body(serde_json::json!({ "id": 7 }));
        });

        let id = client(&server)
            .create_check_run("Badgers diff coverage", "abc123", "Covered", "No gaps", &[])
            .unwrap();
        assert_eq!(id, 7);
        create.assert();
    }

    #[test]
    fn annotation_messages_agree_with_line_ranges() {
        assert_eq!(
            CheckAnnotation::warning("src/lib.rs", 7, 7).message,
            "Changed executable line 7 is not covered."
        );
        assert_eq!(
            CheckAnnotation::warning("src/lib.rs", 7, 9).message,
            "Changed executable lines 7-9 are not covered."
        );
    }
}
