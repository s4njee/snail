//! App identity and on-disk locations (plan.md §1.5, E0.5).
//!
//! Bundle id `dev.snail.app` fixes the directory name on all three platforms, so the store is in
//! the same place regardless of what the executable is called. `SNAIL_CONFIG_DIR` /
//! `SNAIL_CACHE_DIR` / `SNAIL_LOG_DIR` override everything, which is what keeps benchmarks,
//! fixtures and screenshot runs off the real mailbox (E0.9, E17.7).
//!
//! **Cache is separate from config on purpose.** Message bodies and attachments are multi-gigabyte
//! and are a cache, not data; keeping them out of Application Support stops them being
//! iCloud-backed-up.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Mutex;

/// The bundle id. Directory name is fixed to this on macOS, Linux and Windows.
pub const APP_ID: &str = "dev.snail.app";

const CONFIG_ENV: &str = "SNAIL_CONFIG_DIR";
const CACHE_ENV: &str = "SNAIL_CACHE_DIR";
const LOG_ENV: &str = "SNAIL_LOG_DIR";

const LOG_FILE: &str = "snail.log";
const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// The resolved on-disk locations. Resolve once at startup and pass it around; never re-read the
/// environment from inside a view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// Settings JSON, the SQLite store, anything the user would be sad to lose.
    pub config: PathBuf,
    /// Message bodies, attachments, avatars, fetched images. Safe to delete.
    pub cache: PathBuf,
    /// Rotating log files.
    pub log: PathBuf,
}

impl Paths {
    /// Resolve from the environment, falling back to the platform's config/cache dirs.
    pub fn resolve() -> Self {
        Self::from_env(|key| std::env::var_os(key))
    }

    /// Injectable-environment form, so tests never race on process globals.
    pub fn from_env(mut get: impl FnMut(&str) -> Option<OsString>) -> Self {
        let config = override_for(CONFIG_ENV, &mut get)
            .or_else(|| dirs::config_dir().map(|d| d.join(APP_ID)))
            .unwrap_or_else(|| PathBuf::from(".snail/config"));
        let cache = override_for(CACHE_ENV, &mut get)
            .or_else(|| dirs::cache_dir().map(|d| d.join(APP_ID)))
            .unwrap_or_else(|| PathBuf::from(".snail/cache"));
        // Logs default next to the cache, which is never iCloud-backed-up.
        let log = override_for(LOG_ENV, &mut get).unwrap_or_else(|| cache.join("logs"));
        Self { config, cache, log }
    }

    /// The SQLite store.
    pub fn store_db(&self) -> PathBuf {
        self.config.join("snail.sqlite")
    }

    /// Content-addressed message bodies and attachments (E2.3).
    pub fn cache_files(&self) -> PathBuf {
        self.cache.join("files")
    }

    /// One versioned JSON file per settings section (E2.7).
    pub fn settings_file(&self, section: &str) -> PathBuf {
        self.config.join(format!("{section}.json"))
    }

    /// The active log file.
    pub fn log_file(&self) -> PathBuf {
        self.log.join(LOG_FILE)
    }

    /// Create every directory. Called before anything opens a store or a log.
    pub fn ensure(&self) -> io::Result<()> {
        for dir in [&self.config, &self.cache, &self.log] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

fn override_for(key: &str, get: &mut impl FnMut(&str) -> Option<OsString>) -> Option<PathBuf> {
    get(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Install the logger, writing to a size-rotating file and, when `RUST_LOG` is set, stderr as well.
///
/// Cheap enough to leave on permanently (E0.7 needs exactly this).
pub fn init_logging(paths: &Paths) -> io::Result<()> {
    paths.ensure()?;
    let writer = RotatingWriter::new(paths.log_file(), LOG_MAX_BYTES)?;
    let mut builder = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    );
    builder.target(env_logger::Target::Pipe(Box::new(writer)));
    builder
        .try_init()
        .map_err(|error| io::Error::new(io::ErrorKind::AlreadyExists, error.to_string()))
}

/// A `Write` sink that renames the current file aside once it passes `max_bytes`.
struct RotatingWriter {
    path: PathBuf,
    max_bytes: u64,
    state: Mutex<RotatingState>,
}

struct RotatingState {
    file: Option<std::fs::File>,
    written: u64,
}

impl RotatingWriter {
    fn new(path: PathBuf, max_bytes: u64) -> io::Result<Self> {
        let written = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        Ok(Self {
            path,
            max_bytes,
            state: Mutex::new(RotatingState {
                file: None,
                written,
            }),
        })
    }

    fn rotate_locked(&self, state: &mut RotatingState) -> io::Result<()> {
        state.file = None;
        let rotated = self.path.with_extension("log.1");
        // A failed rename must not lose the new record; fall through and keep appending.
        let _ = std::fs::rename(&self.path, rotated);
        state.written = 0;
        Ok(())
    }
}

impl Write for RotatingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut state = self.state.lock().expect("log lock poisoned");
        if state.written + buf.len() as u64 > self.max_bytes {
            self.rotate_locked(&mut state)?;
        }
        if state.file.is_none() {
            state.file = Some(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
            );
        }
        let file = state.file.as_mut().expect("just opened");
        let written = file.write(buf)?;
        state.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut state = self.state.lock().expect("log lock poisoned");
        match state.file.as_mut() {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> impl FnMut(&str) -> Option<OsString> {
        |_| None
    }

    #[test]
    fn overrides_win() {
        let paths = Paths::from_env(|key| match key {
            CONFIG_ENV => Some(OsString::from("/tmp/snail-cfg")),
            CACHE_ENV => Some(OsString::from("/tmp/snail-cache")),
            LOG_ENV => Some(OsString::from("/tmp/snail-log")),
            _ => None,
        });
        assert_eq!(paths.config, PathBuf::from("/tmp/snail-cfg"));
        assert_eq!(paths.cache, PathBuf::from("/tmp/snail-cache"));
        assert_eq!(paths.log, PathBuf::from("/tmp/snail-log"));
    }

    #[test]
    fn empty_override_is_ignored() {
        let paths = Paths::from_env(|key| {
            (key == CONFIG_ENV).then(|| OsString::from(""))
        });
        assert_ne!(paths.config, PathBuf::from(""));
    }

    #[test]
    fn log_defaults_inside_cache_so_it_is_not_backed_up() {
        let paths = Paths::from_env(empty());
        assert_eq!(paths.log, paths.cache.join("logs"));
    }

    #[test]
    fn cache_and_config_are_distinct() {
        let paths = Paths::from_env(empty());
        assert_ne!(paths.config, paths.cache);
        assert_ne!(paths.store_db(), paths.cache_files());
    }

    #[test]
    fn bundle_id_fixes_every_default_path() {
        let paths = Paths::from_env(empty());
        assert!(paths.config.ends_with(APP_ID), "config: {:?}", paths.config);
        assert!(paths.cache.ends_with(APP_ID), "cache: {:?}", paths.cache);
    }

    #[test]
    fn ensure_creates_every_directory() {
        let base = std::env::temp_dir().join(format!("snail-paths-test-{}", std::process::id()));
        let paths = Paths {
            config: base.join("config"),
            cache: base.join("cache"),
            log: base.join("cache/logs"),
        };
        paths.ensure().expect("dirs created");
        assert!(paths.config.is_dir());
        assert!(paths.cache.is_dir());
        assert!(paths.log.is_dir());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rotating_writer_moves_the_old_file_aside() {
        let base = std::env::temp_dir().join(format!("snail-rot-test-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let path = base.join(LOG_FILE);
        let mut writer = RotatingWriter::new(path.clone(), 32).unwrap();
        writer.write_all(&[b'x'; 24]).unwrap();
        writer.write_all(&[b'y'; 24]).unwrap(); // pushes past the cap, rotates
        drop(writer);
        let rotated = path.with_extension("log.1");
        assert!(rotated.is_file(), "old log renamed aside");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 24);
        let _ = std::fs::remove_dir_all(&base);
    }
}
