use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};

use crate::{PutOptions, StorageBackend, StorageError, StoredObject};

/// GCS requires object names in URL paths to be percent-encoded except for
/// RFC 3986 unreserved characters — notably `/` becomes `%2F`.
const OBJECT_ESCAPE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

const MAX_ATTEMPTS: u32 = 3;

/// GCS JSON API client (no SDK): the needed surface is only object
/// get/insert/metadata, and auth is a pre-issued OAuth2 access token.
pub struct GcsBackend {
    client: Client,
    bucket: String,
    token: String,
    base_url: String,
}

impl GcsBackend {
    pub fn new(bucket: impl Into<String>, token: impl Into<String>) -> Self {
        Self::with_base_url(bucket, token, "https://storage.googleapis.com")
    }

    pub fn with_base_url(
        bucket: impl Into<String>,
        token: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            client: Client::new(),
            bucket: bucket.into(),
            token: token.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    fn object_url(&self, key: &str) -> String {
        format!(
            "{}/storage/v1/b/{}/o/{}",
            self.base_url,
            self.bucket,
            utf8_percent_encode(key, OBJECT_ESCAPE)
        )
    }

    fn upload_url(&self, key: &str, if_generation_match: Option<i64>) -> String {
        let mut url = format!(
            "{}/upload/storage/v1/b/{}/o?uploadType=media&name={}",
            self.base_url,
            self.bucket,
            utf8_percent_encode(key, OBJECT_ESCAPE)
        );
        if let Some(generation) = if_generation_match {
            url.push_str(&format!("&ifGenerationMatch={generation}"));
        }
        url
    }

    fn send_with_retry(
        &self,
        key: &str,
        request: impl Fn() -> reqwest::blocking::RequestBuilder,
    ) -> Result<Response, StorageError> {
        let mut last: Option<StorageError> = None;
        for attempt in 0..MAX_ATTEMPTS {
            match request().bearer_auth(&self.token).send() {
                Ok(resp) => {
                    let status = resp.status();
                    let retryable = status.is_server_error()
                        || status == StatusCode::TOO_MANY_REQUESTS
                        || status == StatusCode::REQUEST_TIMEOUT;
                    if !retryable {
                        return Ok(resp);
                    }
                    last = Some(StorageError::UnexpectedStatus {
                        key: key.to_string(),
                        status: status.as_u16(),
                        body: resp.text().unwrap_or_default(),
                    });
                }
                Err(e) => {
                    last = Some(StorageError::Http {
                        key: key.to_string(),
                        message: e.to_string(),
                    });
                }
            }
            if attempt + 1 < MAX_ATTEMPTS {
                backoff_sleep(attempt);
            }
        }
        Err(last.expect("at least one attempt recorded an error"))
    }
}

fn backoff_sleep(attempt: u32) {
    let jitter_ms = u64::from(std::process::id()) % 100;
    let ms = 200u64 * 2u64.pow(attempt) + jitter_ms;
    std::thread::sleep(std::time::Duration::from_millis(ms));
}

fn unexpected(key: &str, resp: Response) -> StorageError {
    StorageError::UnexpectedStatus {
        key: key.to_string(),
        status: resp.status().as_u16(),
        body: resp.text().unwrap_or_default(),
    }
}

impl StorageBackend for GcsBackend {
    fn get(&self, key: &str) -> Result<Option<StoredObject>, StorageError> {
        let url = format!("{}?alt=media", self.object_url(key));
        let resp = self.send_with_retry(key, || self.client.get(&url))?;
        match resp.status() {
            StatusCode::OK => {
                let generation = resp
                    .headers()
                    .get("x-goog-generation")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(0);
                let data = resp
                    .bytes()
                    .map_err(|e| StorageError::Http {
                        key: key.to_string(),
                        message: e.to_string(),
                    })?
                    .to_vec();
                Ok(Some(StoredObject { data, generation }))
            }
            StatusCode::NOT_FOUND => Ok(None),
            _ => Err(unexpected(key, resp)),
        }
    }

    fn put(&self, key: &str, data: &[u8], opts: PutOptions) -> Result<(), StorageError> {
        let url = self.upload_url(key, opts.if_generation_match);
        let resp = self.send_with_retry(key, || {
            self.client
                .post(&url)
                .header("content-type", opts.content_type)
                .body(data.to_vec())
        })?;
        match resp.status() {
            StatusCode::OK => Ok(()),
            StatusCode::PRECONDITION_FAILED => Err(StorageError::PreconditionFailed {
                key: key.to_string(),
            }),
            _ => Err(unexpected(key, resp)),
        }
    }

    fn exists(&self, key: &str) -> Result<bool, StorageError> {
        let url = self.object_url(key);
        let resp = self.send_with_retry(key, || self.client.get(&url))?;
        match resp.status() {
            StatusCode::OK => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            _ => Err(unexpected(key, resp)),
        }
    }
}

#[cfg(test)]
mod tests {
    use httpmock::prelude::*;

    use super::*;
    use crate::{BranchPointer, POINTER_SCHEMA_VERSION, PointerUpdate, update_pointer_if_newer};

    fn backend(server: &MockServer) -> GcsBackend {
        GcsBackend::with_base_url("bkt", "test-token", server.base_url())
    }

    #[test]
    fn get_decodes_generation_and_percent_encodes_key() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/storage/v1/b/bkt/o/badgers%2Frepos%2Fo%2Fr%2Frefs%2Fmain%2Flatest.json")
                .query_param("alt", "media")
                .header("authorization", "Bearer test-token");
            then.status(200)
                .header("x-goog-generation", "42")
                .body("{}");
        });

        let obj = backend(&server)
            .get("badgers/repos/o/r/refs/main/latest.json")
            .unwrap()
            .unwrap();
        mock.assert();
        assert_eq!(obj.generation, 42);
        assert_eq!(obj.data, b"{}");
    }

    #[test]
    fn get_missing_returns_none_and_exists_false() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path_contains("/o/missing.json");
            then.status(404);
        });

        let b = backend(&server);
        assert_eq!(b.get("missing.json").unwrap(), None);
        assert!(!b.exists("missing.json").unwrap());
    }

    #[test]
    fn put_sends_generation_precondition_and_maps_412() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/upload/storage/v1/b/bkt/o")
                .query_param("uploadType", "media")
                .query_param("name", "refs/main/latest.json")
                .query_param("ifGenerationMatch", "42")
                .header("content-type", "application/json");
            then.status(412);
        });

        let err = backend(&server)
            .put(
                "refs/main/latest.json",
                b"{}",
                PutOptions {
                    if_generation_match: Some(42),
                    content_type: "application/json",
                },
            )
            .unwrap_err();
        mock.assert();
        assert!(matches!(err, StorageError::PreconditionFailed { .. }));
    }

    #[test]
    fn server_errors_are_retried_then_surfaced() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path_contains("/o/flaky.json");
            then.status(503).body("unavailable");
        });

        let err = backend(&server).get("flaky.json").unwrap_err();
        assert!(matches!(
            err,
            StorageError::UnexpectedStatus { status: 503, .. }
        ));
        mock.assert_hits(3);
    }

    #[test]
    fn pointer_update_races_are_bounded_by_max_attempts() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path_contains("latest.json");
            then.status(404);
        });
        let put = server.mock(|when, then| {
            when.method(POST).query_param("ifGenerationMatch", "0");
            then.status(412);
        });

        let pointer = BranchPointer {
            schema_version: POINTER_SCHEMA_VERSION,
            branch: "main".to_string(),
            commit_sha: "abc".to_string(),
            committed_at: "2026-07-19T00:00:00Z".to_string(),
            snapshot_key: "commits/abc/coverage.json.zst".to_string(),
            comparison_key: None,
            report_key: None,
            html_prefix: None,
            updated_at: "2026-07-19T00:00:00Z".to_string(),
        };
        let err = update_pointer_if_newer(&backend(&server), "refs/main/latest.json", &pointer)
            .unwrap_err();
        assert!(matches!(err, StorageError::PreconditionFailed { .. }));
        put.assert_hits(3);
    }

    #[test]
    fn pointer_update_succeeds_end_to_end() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path_contains("latest.json");
            then.status(404);
        });
        let put = server.mock(|when, then| {
            when.method(POST).query_param("ifGenerationMatch", "0");
            then.status(200).body("{}");
        });

        let pointer = BranchPointer {
            schema_version: POINTER_SCHEMA_VERSION,
            branch: "main".to_string(),
            commit_sha: "abc".to_string(),
            committed_at: "2026-07-19T00:00:00Z".to_string(),
            snapshot_key: "commits/abc/coverage.json.zst".to_string(),
            comparison_key: None,
            report_key: None,
            html_prefix: None,
            updated_at: "2026-07-19T00:00:00Z".to_string(),
        };
        let update =
            update_pointer_if_newer(&backend(&server), "refs/main/latest.json", &pointer).unwrap();
        assert_eq!(update, PointerUpdate::Updated);
        put.assert();
    }
}
