//! Content-addressed message bodies and attachments (plan.md E2.3). Files live in the **cache**
//! dir, not Application Support, so a multi-gigabyte mail store is never iCloud-backed-up.
//!
//! The store keeps only hashes; a missing file is not an error, it is a signal to re-fetch.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use sha2::{Digest, Sha256};

/// Distinguishes concurrent writers' temp files, so two threads putting the same bytes never
/// rename each other's half-written file (which failed with `NotFound` under parallel tests).
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct CacheStore {
    root: PathBuf,
}

impl CacheStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// A unique sibling temp path for an atomic write. `path`'s own name plus a per-process,
    /// per-call suffix, so writers cannot collide.
    fn temp_path(path: &Path) -> PathBuf {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut name = path
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_default();
        name.push(format!(".tmp-{}-{counter}", std::process::id()));
        path.with_file_name(name)
    }

    /// Write bytes if absent, returning their content hash. Idempotent, and atomic (temp+rename) so
    /// a crash never leaves a half-written object.
    pub fn put(&self, bytes: &[u8]) -> Result<String> {
        let hash = hash_bytes(bytes);
        let path = self.path(&hash);
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let temp = Self::temp_path(&path);
            std::fs::write(&temp, bytes)?;
            std::fs::rename(&temp, &path)?;
        }
        Ok(hash)
    }

    /// The bytes for a hash, or `None` if the file is gone (the caller re-fetches).
    pub fn get(&self, hash: &str) -> Result<Option<Vec<u8>>> {
        let path = self.path(hash);
        if path.is_file() {
            Ok(Some(std::fs::read(path)?))
        } else {
            Ok(None)
        }
    }

    /// Store an object content-addressed and remember an independent lookup key. Remote image URLs
    /// use this so the bytes are deduplicated by content while repeat loads can resolve the URL
    /// without another network request (E6.8).
    pub fn put_alias(&self, key: &str, bytes: &[u8]) -> Result<String> {
        let content_hash = self.put(bytes)?;
        let alias_hash = hash_bytes(key.as_bytes());
        let path = self.alias_path(&alias_hash);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = Self::temp_path(&path);
        std::fs::write(&temp, &content_hash)?;
        std::fs::rename(temp, path)?;
        Ok(content_hash)
    }

    pub fn get_alias(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let alias_hash = hash_bytes(key.as_bytes());
        let path = self.alias_path(&alias_hash);
        if !path.is_file() {
            return Ok(None);
        }
        let content_hash = std::fs::read_to_string(path)?;
        self.get(content_hash.trim())
    }

    /// Delete one object ("clear cache" for a single message).
    pub fn remove(&self, hash: &str) -> Result<()> {
        let path = self.path(hash);
        if path.is_file() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    /// Delete the whole cache — a directory delete, which is the point of the split (E2.3).
    pub fn clear(&self) -> Result<()> {
        if self.root.is_dir() {
            std::fs::remove_dir_all(&self.root)?;
        }
        Ok(())
    }

    /// `<root>/aa/rest`, so no directory grows without bound.
    pub fn path(&self, hash: &str) -> PathBuf {
        let split = hash.len().min(2);
        let (prefix, rest) = hash.split_at(split);
        self.root.join(prefix).join(rest)
    }

    fn alias_path(&self, hash: &str) -> PathBuf {
        let split = hash.len().min(2);
        let (prefix, rest) = hash.split_at(split);
        self.root.join("aliases").join(prefix).join(rest)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// The content hash: SHA-256, lowercase hex.
pub fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "snail-cache-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn put_is_content_addressed_and_idempotent() {
        let dir = temp_dir();
        let cache = CacheStore::new(dir.clone());
        let a = cache.put(b"hello").unwrap();
        let b = cache.put(b"hello").unwrap();
        let c = cache.put(b"world").unwrap();
        assert_eq!(a, b, "same bytes, same hash");
        assert_ne!(a, c);
        assert_eq!(cache.get(&a).unwrap().unwrap(), b"hello");
        assert!(cache.path(&a).starts_with(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_object_is_none_not_an_error() {
        let dir = temp_dir();
        let cache = CacheStore::new(dir.clone());
        let hash = cache.put(b"x").unwrap();
        cache.remove(&hash).unwrap();
        assert_eq!(cache.get(&hash).unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_puts_of_the_same_bytes_do_not_race() {
        // Regression: a fixed `<hash>.tmp` name let two threads rename the same temp file, so one
        // got `NotFound` — which is what macOS CI's parallel tests hit.
        let dir = temp_dir();
        let cache = CacheStore::new(dir.clone());
        let threads = (0..16)
            .map(|_| {
                let cache = cache.clone();
                std::thread::spawn(move || cache.put(b"same bytes").unwrap())
            })
            .collect::<Vec<_>>();
        for thread in threads {
            assert_eq!(thread.join().unwrap(), hash_bytes(b"same bytes"));
        }
        assert_eq!(
            cache.get(&hash_bytes(b"same bytes")).unwrap().unwrap(),
            b"same bytes"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_removes_the_whole_tree() {
        let dir = temp_dir();
        let cache = CacheStore::new(dir.clone());
        cache.put(b"x").unwrap();
        cache.clear().unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn aliases_resolve_to_content_addressed_objects() {
        let dir = temp_dir();
        let cache = CacheStore::new(dir.clone());
        let hash = cache
            .put_alias("https://example.test/image.png", b"pixels")
            .unwrap();
        assert_eq!(hash, hash_bytes(b"pixels"));
        assert_eq!(
            cache
                .get_alias("https://example.test/image.png")
                .unwrap()
                .as_deref(),
            Some(b"pixels".as_slice())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
