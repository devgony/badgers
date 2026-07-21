use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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
            && !key.contains('\\')
            && !key
                .split('/')
                .any(|seg| seg.is_empty() || seg == "." || seg == "..");
        if !valid {
            return Err(StorageError::Http {
                key: key.to_string(),
                message: "invalid object key".to_string(),
            });
        }
        Ok(self.root.join(key))
    }

    fn ensure_no_symlinks(&self, key: &str, path: &Path) -> Result<(), StorageError> {
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| StorageError::Http {
                key: key.to_string(),
                message: "object path escaped local storage root".to_string(),
            })?;
        let mut current = self.root.clone();
        check_component(key, &current, true)?;
        for component in relative.components() {
            current.push(component);
            if !check_component(key, &current, false)? {
                break;
            }
        }
        Ok(())
    }

    fn create_safe_parent_dirs(&self, key: &str, parent: &Path) -> Result<(), StorageError> {
        if !self.root.exists() {
            std::fs::create_dir_all(&self.root).map_err(|e| io_err(key, e))?;
        }
        check_component(key, &self.root, true)?;
        let relative = parent
            .strip_prefix(&self.root)
            .map_err(|_| StorageError::Http {
                key: key.to_string(),
                message: "object parent escaped local storage root".to_string(),
            })?;
        let mut current = self.root.clone();
        for component in relative.components() {
            current.push(component);
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(symlink_err(key, &current));
                }
                Ok(metadata) if !metadata.is_dir() => {
                    return Err(StorageError::Http {
                        key: key.to_string(),
                        message: format!(
                            "storage path component is not a directory: {}",
                            current.display()
                        ),
                    });
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::create_dir(&current).map_err(|e| io_err(key, e))?;
                }
                Err(error) => return Err(io_err(key, error)),
            }
        }
        Ok(())
    }
}

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn check_component(key: &str, path: &Path, require_dir: bool) -> Result<bool, StorageError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(symlink_err(key, path)),
        Ok(metadata) if require_dir && !metadata.is_dir() => Err(StorageError::Http {
            key: key.to_string(),
            message: format!("storage root is not a directory: {}", path.display()),
        }),
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_err(key, error)),
    }
}

fn symlink_err(key: &str, path: &Path) -> StorageError {
    StorageError::Http {
        key: key.to_string(),
        message: format!("storage path contains a symlink: {}", path.display()),
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
        self.ensure_no_symlinks(key, &path)?;
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
        self.create_safe_parent_dirs(key, parent)?;
        self.ensure_no_symlinks(key, &path)?;
        let suffix = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_extension(format!("tmp-badgers-{}-{suffix}", std::process::id()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| io_err(key, e))?;
        std::io::Write::write_all(&mut file, data).map_err(|e| io_err(key, e))?;
        std::fs::rename(&tmp, &path).map_err(|e| io_err(key, e))
    }

    fn exists(&self, key: &str) -> Result<bool, StorageError> {
        let path = self.path_for(key)?;
        self.ensure_no_symlinks(key, &path)?;
        Ok(path.is_file())
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
        for key in ["/abs/path", "a/../b", "a/./b", "", "a//b", "..\\escape"] {
            assert!(backend.get(key).is_err(), "key {key:?} should be rejected");
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("badgers")).unwrap();
        let backend = LocalBackend::new(dir.path());
        let key = "badgers/repos/o/r/commits/abc/README.md";

        assert!(backend.put(key, b"unsafe", PutOptions::default()).is_err());
        assert!(
            !outside
                .path()
                .join("repos/o/r/commits/abc/README.md")
                .exists()
        );
    }
}
