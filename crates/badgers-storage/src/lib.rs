//! Storage backends for badgers coverage snapshots.
//!
//! The [`StorageBackend`] trait abstracts the snapshot ledger. Production uses
//! [`GcsBackend`] (GCS JSON API, no SDK); tests and local development use
//! [`LocalBackend`].

mod gcs;
mod keys;
mod local;
mod pointer;

pub use gcs::GcsBackend;
pub use keys::{Keys, encode_branch};
pub use local::LocalBackend;
pub use pointer::{BranchPointer, POINTER_SCHEMA_VERSION, PointerUpdate, update_pointer_if_newer};

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("precondition failed (generation mismatch) for {key}")]
    PreconditionFailed { key: String },
    #[error("storage I/O error for {key}: {source}")]
    Io {
        key: String,
        #[source]
        source: std::io::Error,
    },
    #[error("http error for {key}: {message}")]
    Http { key: String, message: String },
    #[error("unexpected status {status} for {key}: {body}")]
    UnexpectedStatus {
        key: String,
        status: u16,
        body: String,
    },
}

/// An object read from storage together with its generation number, used for
/// conditional writes ([`PutOptions::if_generation_match`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredObject {
    pub data: Vec<u8>,
    /// GCS object generation. `0` on backends without generations (local).
    pub generation: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct PutOptions {
    /// GCS conditional write: only succeed if the current generation matches.
    /// `Some(0)` means "only if the object does not exist yet".
    /// Ignored by [`LocalBackend`].
    pub if_generation_match: Option<i64>,
    pub content_type: &'static str,
}

impl Default for PutOptions {
    fn default() -> Self {
        Self {
            if_generation_match: None,
            content_type: "application/octet-stream",
        }
    }
}

pub trait StorageBackend {
    fn get(&self, key: &str) -> Result<Option<StoredObject>, StorageError>;
    fn put(&self, key: &str, data: &[u8], opts: PutOptions) -> Result<(), StorageError>;
    fn exists(&self, key: &str) -> Result<bool, StorageError>;
}
