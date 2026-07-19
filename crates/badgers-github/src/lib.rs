//! Minimal GitHub REST client for badgers PR reporting.

use serde::Deserialize;

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
}

const PER_PAGE: u32 = 100;
const MAX_PAGES: u32 = 10;

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
            client: reqwest::blocking::Client::new(),
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
        if let Some(comment_id) = self.find_comment(pr_number, marker)? {
            let url = format!(
                "{}/repos/{}/issues/comments/{comment_id}",
                self.base_url, self.repo
            );
            let resp = self
                .request(self.client.patch(&url))
                .json(&serde_json::json!({ "body": body }))
                .send()?;
            expect_success(resp)?;
            Ok(CommentAction::Updated)
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
            Ok(CommentAction::Created)
        }
    }

    fn find_comment(&self, pr_number: u64, marker: &str) -> Result<Option<u64>, GithubError> {
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
                if comment.body.is_some_and(|b| b.contains(marker)) {
                    return Ok(Some(comment.id));
                }
            }
            if count < PER_PAGE as usize {
                break;
            }
        }
        Ok(None)
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
            then.status(200).json_body(serde_json::json!([
                { "id": 7, "body": "<!-- badgers-report:owner/repo:5 -->\nold" }
            ]));
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
}
