//! Storage for badgers coverage snapshots.
//!
//! The [`StorageBackend`] trait abstracts the snapshot ledger;
//! [`LocalBackend`] stores objects under a local directory (for example a
//! checkout of the coverage storage branch).

mod keys;
mod local;
mod pointer;

pub use keys::{Keys, encode_branch};
pub use local::LocalBackend;
pub use pointer::{BranchPointer, POINTER_SCHEMA_VERSION, PointerUpdate, update_pointer_if_newer};

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("storage I/O error for {key}: {source}")]
    Io {
        key: String,
        #[source]
        source: std::io::Error,
    },
    #[error("storage error for {key}: {message}")]
    Invalid { key: String, message: String },
}

/// An object read from storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredObject {
    pub data: Vec<u8>,
}

pub trait StorageBackend {
    fn get(&self, key: &str) -> Result<Option<StoredObject>, StorageError>;
    fn put(&self, key: &str, data: &[u8]) -> Result<(), StorageError>;
    fn exists(&self, key: &str) -> Result<bool, StorageError>;
}
