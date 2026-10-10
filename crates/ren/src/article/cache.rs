// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The images of articles on disk, so that articles read again don't fetch
//! them again and show them offline.
//!
//! One file per URL, named after its hash, holding the URL (to tell hash
//! collisions apart) and the bytes as fetched. The cache is kept below a
//! size by removing the files used least recently: a read sets a file's
//! modification time.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use crate::hash::fnv1a;

/// When the cache grows above its size, files are removed until it is
/// below this share of it, so that not every new file removes one.
const EVICT_TO_PERCENT: u64 = 90;

pub struct ImageCache {
    dir: PathBuf,
    max_bytes: u64,
    /// The size of the files, read from the directory when the first file
    /// is added. Writes from several threads may make it drift a little;
    /// it is counted anew when files are removed.
    total: Mutex<Option<u64>>,
}

impl ImageCache {
    /// A cache in `dir`, created when the first file is added, of at most
    /// `max_bytes`.
    pub fn new(dir: PathBuf, max_bytes: u64) -> Self {
        Self {
            dir,
            max_bytes,
            total: Mutex::new(None),
        }
    }

    fn path(&self, url: &str) -> PathBuf {
        self.dir.join(format!("{:016x}", fnv1a(url)))
    }

    /// The bytes stored for `url`.
    pub fn get(&self, url: &str) -> Option<Vec<u8>> {
        let path = self.path(url);
        let mut file = File::open(&path).ok()?;
        let mut content = Vec::new();
        file.read_to_end(&mut content).ok()?;
        let newline = content.iter().position(|&b| b == b'\n')?;
        if &content[..newline] != url.as_bytes() {
            return None;
        }
        // Used now: the last file to be removed.
        if let Ok(file) = File::options().write(true).open(&path) {
            let _ = file.set_modified(SystemTime::now());
        }
        content.drain(..=newline);
        Some(content)
    }

    /// Stores `bytes` for `url`. Errors are only logged: the cache is an
    /// optimisation.
    pub fn put(&self, url: &str, bytes: &[u8]) {
        if url.contains('\n') {
            return;
        }
        let size = (url.len() + 1 + bytes.len()) as u64;
        if size > self.max_bytes / 4 {
            return;
        }
        // Counted before the new file is there.
        if let Ok(mut total) = self.total.lock() {
            total.get_or_insert_with(|| directory_size(&self.dir));
        }
        let replaced = fs::metadata(self.path(url)).map_or(0, |m| m.len());
        if let Err(err) = self.write(url, bytes) {
            eprintln!("ren: image cache {}: {err}", self.dir.display());
            return;
        }
        let Ok(mut total) = self.total.lock() else {
            return;
        };
        let total = total.get_or_insert(0);
        *total = (*total + size).saturating_sub(replaced);
        if *total > self.max_bytes {
            *total = evict(&self.dir, self.max_bytes * EVICT_TO_PERCENT / 100);
        }
    }

    fn write(&self, url: &str, bytes: &[u8]) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.path(url);
        // Written under another name and renamed, so that a reader never
        // sees half a file.
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let temporary = self.dir.join(format!(
            ".{}.{}.{}",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default(),
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        let written = File::create(&temporary).and_then(|mut file| {
            file.write_all(url.as_bytes())?;
            file.write_all(b"\n")?;
            file.write_all(bytes)
        });
        match written.and_then(|()| fs::rename(&temporary, &path)) {
            Ok(()) => Ok(()),
            Err(err) => {
                let _ = fs::remove_file(&temporary);
                Err(err)
            }
        }
    }
}

/// The files of the cache with their size and time of last use.
fn files(dir: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then(|| {
                let used = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                (entry.path(), metadata.len(), used)
            })
        })
        .collect()
}

fn directory_size(dir: &Path) -> u64 {
    files(dir).iter().map(|(_, size, _)| size).sum()
}

/// Removes the files used least recently until at most `target` bytes are
/// left. Returns what is left.
fn evict(dir: &Path, target: u64) -> u64 {
    let mut files = files(dir);
    let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
    files.sort_by_key(|(_, _, used)| *used);
    for (path, size, _) in files {
        if total <= target {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total -= size;
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A directory removed at the end of the test.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ren-test-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn age(cache: &ImageCache, url: &str, seconds: u64) {
        let file = File::options().write(true).open(cache.path(url)).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(seconds))
            .unwrap();
    }

    #[test]
    fn stores_and_returns_bytes() {
        let dir = TempDir::new("cache-store");
        let cache = ImageCache::new(dir.0.join("images"), 1 << 20);
        assert_eq!(cache.get("https://example.org/a.png"), None);
        cache.put("https://example.org/a.png", b"\x89PNG\n...");
        cache.put("https://example.org/b.png", b"");
        assert_eq!(
            cache.get("https://example.org/a.png").as_deref(),
            Some(&b"\x89PNG\n..."[..])
        );
        assert_eq!(
            cache.get("https://example.org/b.png").as_deref(),
            Some(&[][..])
        );
        // Replaced.
        cache.put("https://example.org/a.png", b"new");
        assert_eq!(
            cache.get("https://example.org/a.png").as_deref(),
            Some(&b"new"[..])
        );
        // A new cache on the same directory.
        let again = ImageCache::new(dir.0.join("images"), 1 << 20);
        assert_eq!(
            again.get("https://example.org/a.png").as_deref(),
            Some(&b"new"[..])
        );
        // No temporary files left.
        assert_eq!(files(&dir.0.join("images")).len(), 2);
    }

    #[test]
    fn another_url_with_the_same_file_is_a_miss() {
        let dir = TempDir::new("cache-collision");
        let cache = ImageCache::new(dir.0.clone(), 1 << 20);
        cache.put("https://example.org/a.png", b"a");
        fs::rename(
            cache.path("https://example.org/a.png"),
            cache.path("https://example.org/b.png"),
        )
        .unwrap();
        assert_eq!(cache.get("https://example.org/b.png"), None);
    }

    #[test]
    fn removes_the_files_used_least_recently() {
        let dir = TempDir::new("cache-evict");
        // Each file is 1,000 bytes with its URL ("u0\n" and 997 bytes).
        let cache = ImageCache::new(dir.0.clone(), 4_500);
        let bytes = [0u8; 997];
        for (n, seconds) in [(0, 40), (1, 10), (2, 30), (3, 20)] {
            let url = format!("u{n}");
            cache.put(&url, &bytes);
            age(&cache, &url, seconds);
        }
        // u0 is the oldest, but used now.
        assert!(cache.get("u0").is_some());
        cache.put("u4", &bytes);
        // 5,000 bytes > 4,500: down to 90 % (4,050), so one file goes, the
        // one used least recently.
        assert!(cache.get("u2").is_none());
        for url in ["u0", "u1", "u3", "u4"] {
            assert!(cache.get(url).is_some(), "{url}");
        }
        assert_eq!(directory_size(&dir.0), 4_000);
    }

    #[test]
    fn large_files_are_not_stored() {
        let dir = TempDir::new("cache-large");
        let cache = ImageCache::new(dir.0.clone(), 4_000);
        cache.put("u", &[0; 1_000]);
        assert_eq!(cache.get("u"), None);
        cache.put("u", &[0; 900]);
        assert!(cache.get("u").is_some());
    }
}
