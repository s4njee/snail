//! Durable atomic file replacement across Snail's supported platforms.
//!
//! `std::fs::rename` cannot replace an existing destination on Windows. Keeping the platform
//! boundary here makes settings, cache aliases, and log rotation use the same semantics.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

const RETRY_DELAYS: [Duration; 6] = [
    Duration::from_millis(10),
    Duration::from_millis(25),
    Duration::from_millis(50),
    Duration::from_millis(100),
    Duration::from_millis(200),
    Duration::from_millis(400),
];

/// Write a complete sibling temp file, flush it, then atomically replace `path`.
///
/// If replacement fails, the previous destination remains intact and the temporary file is
/// removed. Short retries cover transient antivirus/indexer sharing violations on Windows.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let (temp, mut file) = create_temp(path)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        replace_file(&temp, path)
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Atomically move `source` over `destination`, retrying only transient Windows sharing errors.
pub fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    replace_with_retry(source, destination, &RETRY_DELAYS, platform_replace)
}

fn create_temp(path: &Path) -> io::Result<(PathBuf, File)> {
    for _ in 0..100 {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut name = path
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_default();
        name.push(format!(".tmp-{}-{counter}", std::process::id()));
        let temp = path.with_file_name(name);
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => return Ok((temp, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique atomic-write temporary file",
    ))
}

fn replace_with_retry(
    source: &Path,
    destination: &Path,
    delays: &[Duration],
    mut replace: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    for delay in delays {
        match replace(source, destination) {
            Ok(()) => return Ok(()),
            Err(error) if is_transient_replace_error(&error) => std::thread::sleep(*delay),
            Err(error) => return Err(error),
        }
    }
    replace(source, destination)
}

#[cfg(target_os = "windows")]
fn is_transient_replace_error(error: &io::Error) -> bool {
    // Access denied, sharing violation, and lock violation. Virus scanners and indexers commonly
    // hold the destination briefly after observing the temp file.
    matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

#[cfg(not(target_os = "windows"))]
fn is_transient_replace_error(_error: &io::Error) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn platform_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both arguments are owned, NUL-terminated UTF-16 buffers that outlive the call.
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "windows"))]
fn platform_replace(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::rename(source, destination)?;
    if let Some(parent) = destination.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "snail-atomic-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn first_write_and_overwrite_are_atomic() {
        let dir = temp_dir("replace");
        let path = dir.join("settings.json");
        atomic_write(&path, b"old").unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unicode_and_spaces_are_preserved() {
        let dir = temp_dir("unicode")
            .join("long profile prefix with spaces")
            .join("郵便");
        let path = dir.join("thème.json");
        atomic_write(&path, "café 🐌".as_bytes()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "café 🐌");
        let root = dir.ancestors().nth(2).unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn retry_exhaustion_does_not_touch_the_last_good_file() {
        let dir = temp_dir("exhaustion");
        let destination = dir.join("state.json");
        let source = dir.join("state.json.tmp");
        std::fs::write(&destination, b"last good").unwrap();
        std::fs::write(&source, b"new value").unwrap();
        let mut calls = 0;
        #[cfg(target_os = "windows")]
        let delays = [Duration::ZERO, Duration::ZERO];
        #[cfg(not(target_os = "windows"))]
        let delays = [];
        let error = replace_with_retry(&source, &destination, &delays, |_, _| {
            calls += 1;
            #[cfg(target_os = "windows")]
            return Err(io::Error::from_raw_os_error(32));
            #[cfg(not(target_os = "windows"))]
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "busy"))
        })
        .unwrap_err();
        #[cfg(target_os = "windows")]
        assert_eq!(error.raw_os_error(), Some(32));
        #[cfg(not(target_os = "windows"))]
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(calls, delays.len() + 1);
        assert_eq!(std::fs::read(&destination).unwrap(), b"last good");
        assert_eq!(std::fs::read(&source).unwrap(), b"new value");
        let _ = std::fs::remove_dir_all(dir);
    }
}
