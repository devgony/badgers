use std::path::{Path, PathBuf};

use crate::{PutOptions, StorageBackend, StorageError, StoredObject};

/// Filesystem-backed storage for tests and local development.
///
/// Generations are not tracked: reads report generation `0` and
/// [`PutOptions::if_generation_match`] is ignored, per the storage design.
pub struct LocalBackend {
    root: PathBuf,
}

impl LocalBackend {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path_for(&self, key: &str) -> Result<PathBuf, StorageError> {
        let valid = !key.is_empty()
            && !key.starts_with('/')
            && !key.split('/').any(|seg| seg.is_empty() || seg == "..");
        if !valid {
            return Err(StorageError::Http {
                key: key.to_string(),
                message: "invalid object key".to_string(),
            });
        }
        Ok(self.root.join(key))
    }
}

fn io_err(key: &str, source: std::io::Error) -> StorageError {
    StorageError::Io {
        key: key.to_string(),
        source,
    }
}

impl StorageBackend for LocalBackend {
    fn get(&self, key: &str) -> Result<Option<StoredObject>, StorageError> {
        let path = self.path_for(key)?;
        match std::fs::read(&path) {
            Ok(data) => Ok(Some(StoredObject {
                data,
                generation: 0,
            })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err(key, e)),
        }
    }

    fn put(&self, key: &str, data: &[u8], _opts: PutOptions) -> Result<(), StorageError> {
        let path = self.path_for(key)?;
        let parent = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent).map_err(|e| io_err(key, e))?;
        let tmp = path.with_extension("tmp-badgers");
        std::fs::write(&tmp, data).map_err(|e| io_err(key, e))?;
        std::fs::rename(&tmp, &path).map_err(|e| io_err(key, e))
    }

    fn exists(&self, key: &str) -> Result<bool, StorageError> {
        Ok(self.path_for(key)?.is_file())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_get_put_exists() {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path());
        let key = "badgers/repos/o/r/commits/abc/coverage.json.zst";

        assert!(!backend.exists(key).unwrap());
        assert_eq!(backend.get(key).unwrap(), None);

        backend.put(key, b"hello", PutOptions::default()).unwrap();
        assert!(backend.exists(key).unwrap());
        let obj = backend.get(key).unwrap().unwrap();
        assert_eq!(obj.data, b"hello");
        assert_eq!(obj.generation, 0);

        backend.put(key, b"world", PutOptions::default()).unwrap();
        assert_eq!(backend.get(key).unwrap().unwrap().data, b"world");
    }

    #[test]
    fn rejects_escaping_keys() {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path());
        for key in ["/abs/path", "a/../b", "", "a//b"] {
            assert!(backend.get(key).is_err(), "key {key:?} should be rejected");
        }
    }
}
