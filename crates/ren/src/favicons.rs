// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The feeds' icons ("favicons"), from the links the News app found on
//! their sites. Downloaded after a sync on a thread of low priority,
//! decoded (ICO, PNG, JPEG), scaled down to [`SIZE`] and kept as PNG in
//! `$XDG_CACHE_HOME/ren/favicons`, which the feed tree loads them from.
//! An icon is downloaded again when it is [`MAX_AGE`] old; one that failed
//! is tried again after [`RETRY_AFTER`].

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use image::{DynamicImage, ImageFormat, ImageReader, Limits};

use crate::hash::fnv1a;

/// The icons' size in pixels: the tree shows them at 16 logical pixels,
/// so this is enough for a scale factor of 2.
pub const SIZE: u32 = 32;

/// Icons are downloaded again after this time.
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// An icon that couldn't be downloaded or decoded is tried again after
/// this time.
const RETRY_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Larger downloads aren't icons.
const MAX_BYTES: u64 = 512 * 1024;

/// Nor are larger images; checked from the header before decoding.
const MAX_SIDE: u32 = 1024;

/// An update is running; at most one at a time.
static UPDATING: AtomicBool = AtomicBool::new(false);

/// The icon cache.
#[derive(Debug, Clone)]
pub struct Favicons {
    dir: PathBuf,
}

impl Favicons {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The default directory, `$XDG_CACHE_HOME/ren/favicons`.
    pub fn default_dir() -> Option<PathBuf> {
        ren_settings::paths::favicons(|name| std::env::var(name).ok())
    }

    /// Where the icon from `url` is kept.
    pub fn path(&self, url: &str) -> PathBuf {
        self.dir.join(format!("{:016x}.png", fnv1a(url)))
    }

    /// The marker of a failed download of `url`.
    fn failed_path(&self, url: &str) -> PathBuf {
        self.dir.join(format!("{:016x}.failed", fnv1a(url)))
    }

    /// The icon of `url` if it was downloaded.
    pub fn icon(&self, url: &str) -> Option<PathBuf> {
        let path = self.path(url);
        path.exists().then_some(path)
    }

    /// The icons of `urls` to download: missing or older than
    /// [`MAX_AGE`], and not failed within [`RETRY_AFTER`]. Each once, and
    /// only web addresses.
    pub fn due<'a>(&self, urls: impl IntoIterator<Item = &'a str>, now: SystemTime) -> Vec<String> {
        let older_than = |path: &Path, age: Duration| {
            fs::metadata(path)
                .and_then(|meta| meta.modified())
                .map_or(true, |modified| {
                    now.duration_since(modified)
                        .is_ok_and(|elapsed| elapsed >= age)
                })
        };
        let mut due: Vec<String> = urls
            .into_iter()
            .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
            .filter(|url| older_than(&self.path(url), MAX_AGE))
            .filter(|url| older_than(&self.failed_path(url), RETRY_AFTER))
            .map(str::to_owned)
            .collect();
        due.sort_unstable();
        due.dedup();
        due
    }

    /// Downloads `urls` one after another on a thread of low priority;
    /// `done` is called there with whether any icon was written. Returns
    /// false, without downloading, if an update is running already.
    pub fn update(&self, urls: Vec<String>, done: impl FnOnce(bool) + Send + 'static) -> bool {
        if urls.is_empty() || UPDATING.swap(true, Ordering::AcqRel) {
            return false;
        }
        let favicons = self.clone();
        let spawned = std::thread::Builder::new()
            .name("ren-favicons".to_owned())
            .spawn(move || {
                crate::sync_thread::lower_priority();
                let agent = crate::http::agent();
                let mut changed = false;
                for url in &urls {
                    match favicons.download(&agent, url) {
                        Ok(()) => changed = true,
                        Err(err) => eprintln!("ren: the feed icon {url}: {err}"),
                    }
                }
                drop(agent);
                crate::procstat::trim_heap();
                UPDATING.store(false, Ordering::Release);
                done(changed);
            });
        if spawned.is_err() {
            UPDATING.store(false, Ordering::Release);
            return false;
        }
        true
    }

    /// Downloads the icon from `url` and writes it as PNG. A failure is
    /// marked, so it isn't tried again at every sync, and keeps an older
    /// icon.
    fn download(&self, agent: &ureq::Agent, url: &str) -> Result<(), String> {
        fs::create_dir_all(&self.dir).map_err(|err| format!("{}: {err}", self.dir.display()))?;
        let result = crate::http::fetch(agent, url, MAX_BYTES).and_then(|bytes| convert(&bytes));
        let failed = self.failed_path(url);
        match result {
            Ok(png) => {
                let _ = fs::remove_file(&failed);
                // Written next to it and renamed, so the tree never loads
                // half an icon.
                let path = self.path(url);
                let temporary = path.with_extension("png.tmp");
                fs::write(&temporary, png)
                    .and_then(|()| fs::rename(&temporary, &path))
                    .map_err(|err| format!("{}: {err}", path.display()))
            }
            Err(err) => {
                let _ = fs::write(&failed, b"");
                Err(err)
            }
        }
    }
}

/// Decodes an icon, scales it down to [`SIZE`] if it is larger, and
/// encodes it as PNG.
pub fn convert(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|err| err.to_string())?;
    if reader.format().is_none() {
        return Err("not an image in a known format".to_owned());
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    reader.limits(limits);
    let image = reader.decode().map_err(|err| err.to_string())?;
    let image = if image.width() > SIZE || image.height() > SIZE {
        image.resize(SIZE, SIZE, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let image = DynamicImage::ImageRgba8(image.into_rgba8());
    let mut png = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|err| err.to_string())?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square PNG of `side` pixels.
    fn png(side: u32) -> Vec<u8> {
        let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            side,
            side,
            image::Rgba([200, 30, 30, 255]),
        ));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    /// An ICO with one 16×16 entry of 32 bits per pixel, as most
    /// favicon.ico files are.
    fn ico() -> Vec<u8> {
        let mut bytes = Vec::new();
        // Header: reserved, type 1 (icon), one image.
        bytes.extend([0, 0, 1, 0, 1, 0]);
        let pixels = 16 * 16 * 4;
        let mask = 16 * 4;
        let size: u32 = 40 + pixels + mask;
        // Directory entry: 16×16, no palette, 1 plane, 32 bpp, size, offset.
        bytes.extend([16, 16, 0, 0, 1, 0, 32, 0]);
        bytes.extend(size.to_le_bytes());
        bytes.extend(22u32.to_le_bytes());
        // BITMAPINFOHEADER, twice the height for the mask.
        bytes.extend(40u32.to_le_bytes());
        bytes.extend(16i32.to_le_bytes());
        bytes.extend(32i32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(32u16.to_le_bytes());
        bytes.extend([0; 24]);
        for _ in 0..16 * 16 {
            bytes.extend([30, 30, 200, 255]);
        }
        bytes.extend(std::iter::repeat_n(0, mask as usize));
        bytes
    }

    fn decoded(png: &[u8]) -> image::RgbaImage {
        image::load_from_memory_with_format(png, ImageFormat::Png)
            .unwrap()
            .into_rgba8()
    }

    #[test]
    fn icons_converted() {
        let small = decoded(&convert(&png(16)).unwrap());
        assert_eq!(small.dimensions(), (16, 16));
        let large = decoded(&convert(&png(256)).unwrap());
        assert_eq!(large.dimensions(), (SIZE, SIZE));
        assert_eq!(large.get_pixel(5, 5), &image::Rgba([200, 30, 30, 255]));

        let ico = decoded(&convert(&ico()).unwrap());
        assert_eq!(ico.dimensions(), (16, 16));
        assert_eq!(ico.get_pixel(0, 0), &image::Rgba([200, 30, 30, 255]));

        assert!(convert(b"<html>Not found</html>").is_err());
        assert!(convert(&png(MAX_SIDE + 1)).is_err());
    }

    #[test]
    fn due_icons() {
        let dir = std::env::temp_dir().join(format!("ren-favicons-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let favicons = Favicons::new(dir.clone());
        let now = SystemTime::now();
        let urls = [
            "https://a.example/favicon.ico",
            "https://b.example/icon.png",
            "https://a.example/favicon.ico",
            "file:///etc/passwd",
            "",
        ];
        assert_eq!(
            favicons.due(urls, now),
            [
                "https://a.example/favicon.ico",
                "https://b.example/icon.png"
            ]
        );
        fs::write(favicons.path(urls[0]), png(16)).unwrap();
        fs::write(favicons.failed_path(urls[1]), b"").unwrap();
        assert!(favicons.due(urls, now).is_empty());
        assert_eq!(favicons.icon(urls[0]), Some(favicons.path(urls[0])));
        assert_eq!(favicons.icon(urls[1]), None);

        // A month later the icon is old, and the failure is retried (a
        // minute more, as the files were written after `now`).
        let margin = Duration::from_secs(60);
        let later = now + MAX_AGE + margin;
        assert_eq!(
            favicons.due(urls, later),
            [
                "https://a.example/favicon.ico",
                "https://b.example/icon.png"
            ]
        );
        let tomorrow = now + RETRY_AFTER + margin;
        assert_eq!(favicons.due(urls, tomorrow), ["https://b.example/icon.png"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn downloads() {
        use crate::testing::mock::{MockServer, Response};
        let server = MockServer::start(|request| match request.path.as_str() {
            "/icon.png" => Response {
                content_type: "image/png",
                ..Response::status(200, png(64))
            },
            "/large.png" => Response::status(200, vec![0; MAX_BYTES as usize + 1]),
            _ => Response::html(404, "Not found"),
        });
        let dir = crate::testing::TempDir::new("favicons-download");
        let favicons = Favicons::new(dir.join("icons"));
        let agent = crate::http::agent();
        let icon = format!("{}/icon.png", server.url());
        let missing = format!("{}/missing.ico", server.url());

        favicons.download(&agent, &icon).unwrap();
        let written = decoded(&fs::read(favicons.path(&icon)).unwrap());
        assert_eq!(written.dimensions(), (SIZE, SIZE));
        // Failures are marked, and not tried again for a while.
        assert!(favicons.download(&agent, &missing).is_err());
        assert!(favicons.failed_path(&missing).exists());
        let large = format!("{}/large.png", server.url());
        assert!(favicons.download(&agent, &large).is_err());
        assert!(
            favicons
                .due([icon.as_str(), &missing, &large], SystemTime::now())
                .is_empty()
        );

        // On a thread, one update at a time; nothing new is no change.
        let (sender, done) = std::sync::mpsc::channel();
        assert!(!favicons.update(Vec::new(), |_| {}));
        assert!(favicons.update(vec![missing], move |changed| {
            sender.send(changed).unwrap();
        }));
        assert!(!done.recv_timeout(Duration::from_secs(10)).unwrap());
    }
}
