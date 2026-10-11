// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Content blocking: requests of pages are checked against filter lists
//! with Brave's adblock engine and cancelled if they are ads or trackers.
//! Network rules only; see `docs/decisions/0012-content-blocking.md`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::UNIX_EPOCH;

use adblock::Engine;
use adblock::lists::{FilterSet, ParseOptions, RuleTypes};
use adblock::request::Request;
use ren_tabs::Blocking;
use url::Url;

use crate::browser::Waker;

/// The longest filter list read; the window downloads at most as much.
const MAX_LIST_BYTES: u64 = 16 * 1024 * 1024;

/// Separates the description of the lists from the engine in the cache.
const CACHE_SEPARATOR: u8 = 0;

/// The engine and what it blocks, on the helper's main thread.
#[derive(Default)]
pub struct Blocker {
    config: Option<Blocking>,
    engine: Option<Engine>,
    /// The engine being made for `config`'s lists.
    loading: Option<mpsc::Receiver<Option<Engine>>>,
    /// Requests checked and blocked since the last `take_counts`.
    checked: u32,
    blocked: u32,
}

/// What to do with a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Block,
    /// The engine isn't loaded yet; ask again when `poll` returns true.
    Wait,
}

impl Blocker {
    /// Blocks with `config` from now on; loads its lists on a thread if
    /// they differ from those of the engine in use, which blocks until
    /// then.
    pub fn configure(&mut self, config: Option<Blocking>, waker: &Waker) {
        let Some(new) = config else {
            self.config = None;
            self.engine = None;
            self.loading = None;
            return;
        };
        let reload = lists_changed(&new, &self.config);
        self.config = Some(new.clone());
        if !reload {
            return;
        }
        let config = new;
        let (sender, receiver) = mpsc::channel();
        let waker = waker.clone();
        let spawned = std::thread::Builder::new()
            .name("ren-servo-filters".to_owned())
            .spawn(move || {
                let engine = match load_engine(&config) {
                    Ok(engine) => Some(engine),
                    Err(err) => {
                        eprintln!("ren-servo: no content blocking: {err}");
                        None
                    }
                };
                let _ = sender.send(engine);
                waker();
            });
        match spawned {
            Ok(_) => self.loading = Some(receiver),
            Err(err) => eprintln!("ren-servo: no content blocking: {err}"),
        }
    }

    /// Takes the engine if it was loaded; returns whether it was.
    pub fn poll(&mut self) -> bool {
        let Some(receiver) = &self.loading else {
            return false;
        };
        match receiver.try_recv() {
            Ok(engine) => {
                self.loading = None;
                // A failed load keeps the old engine, if there is one.
                if engine.is_some() {
                    self.engine = engine;
                }
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.loading = None;
                true
            }
        }
    }

    /// Whether a request for `url` should be blocked. `page` is the
    /// document it's made for, if known, `tab` the URL of its tab, and
    /// `destination` its fetch destination (`script`, `image`, … or
    /// empty). Nothing is blocked if either is on a site that is an
    /// exception.
    pub fn check(
        &mut self,
        url: &Url,
        page: Option<&Url>,
        tab: Option<&Url>,
        destination: &str,
    ) -> Verdict {
        let Some(config) = &self.config else {
            return Verdict::Allow;
        };
        if [page, tab]
            .into_iter()
            .flatten()
            .filter_map(Url::host_str)
            .any(|host| is_unblocked(&config.unblocked, host))
        {
            return Verdict::Allow;
        }
        let page = page.or(tab);
        let Some(engine) = &self.engine else {
            return if self.loading.is_some() {
                Verdict::Wait
            } else {
                Verdict::Allow
            };
        };
        self.checked = self.checked.saturating_add(1);
        if should_block(engine, url, page, destination) {
            self.blocked = self.blocked.saturating_add(1);
            Verdict::Block
        } else {
            Verdict::Allow
        }
    }

    /// Whether requests are checked without waiting: the engine is loaded,
    /// or none is being loaded.
    pub fn is_ready(&self) -> bool {
        self.engine.is_some() || self.loading.is_none()
    }

    /// The requests checked and blocked since the last call.
    pub fn take_counts(&mut self) -> (u32, u32) {
        (
            std::mem::take(&mut self.checked),
            std::mem::take(&mut self.blocked),
        )
    }
}

/// Whether the engine blocks a request for `url` from `page`.
pub fn should_block(engine: &Engine, url: &Url, page: Option<&Url>, destination: &str) -> bool {
    let source = page.unwrap_or(url);
    Request::new(
        url.as_str(),
        source.as_str(),
        request_type(destination),
        "GET",
    )
    .is_ok_and(|request| engine.check_network_request(&request).should_block())
}

/// The engine's request type for a fetch destination
/// (<https://fetch.spec.whatwg.org/#concept-request-destination>).
pub fn request_type(destination: &str) -> &'static str {
    match destination {
        "document" => "document",
        "iframe" | "frame" => "subdocument",
        "script" | "worker" | "sharedworker" | "serviceworker" | "audioworklet"
        | "paintworklet" | "xslt" => "script",
        "style" => "stylesheet",
        "image" => "image",
        "font" => "font",
        "audio" | "video" | "track" => "media",
        "object" | "embed" => "object",
        // fetch(), XMLHttpRequest and beacons.
        "" => "xmlhttprequest",
        _ => "other",
    }
}

/// Whether `host` is one of `sites` or a subdomain of one.
pub fn is_unblocked(sites: &[String], host: &str) -> bool {
    let host = host.trim_end_matches('.');
    sites.iter().any(|site| {
        let site = site.trim_end_matches('.');
        !site.is_empty()
            && host.len() >= site.len()
            && host[host.len() - site.len()..].eq_ignore_ascii_case(site)
            && (host.len() == site.len() || host.as_bytes()[host.len() - site.len() - 1] == b'.')
    })
}

/// Whether the list files differ from those `old` was loaded from: other
/// lists, or the same ones changed on disk (the window downloaded new
/// ones).
fn lists_changed(config: &Blocking, old: &Option<Blocking>) -> bool {
    // The engine in use was made from the files as they were then; the
    // cache's key tells whether they changed since.
    match old {
        Some(old) if old.lists == config.lists && old.cache == config.cache => {
            cached_key(&config.cache).as_deref() != Some(lists_key(&config.lists).as_bytes())
        }
        _ => true,
    }
}

/// Describes the list files as they are now: when one is downloaded
/// again, its modification time changes, and the engine is made again.
pub fn lists_key(lists: &[PathBuf]) -> String {
    let mut key = String::new();
    for list in lists {
        let Ok(meta) = fs::metadata(list) else {
            continue;
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |time| time.as_nanos());
        key.push_str(&format!("{}\t{}\t{modified}\n", list.display(), meta.len()));
    }
    key
}

fn cached_key(cache: &Path) -> Option<Vec<u8>> {
    let data = fs::read(cache).ok()?;
    let end = data.iter().position(|&b| b == CACHE_SEPARATOR)?;
    Some(data[..end].to_vec())
}

/// The engine for `config`'s lists: from the cache if it was made from
/// the same files, else made from the lists and saved there.
fn load_engine(config: &Blocking) -> Result<Engine, String> {
    let key = lists_key(&config.lists);
    if key.is_empty() {
        return Err("no filter lists yet".to_owned());
    }
    if let Some(engine) = read_cache(&config.cache, &key) {
        return Ok(engine);
    }
    let engine = build_engine(&config.lists)?;
    if let Err(err) = write_cache(&config.cache, &key, &engine) {
        eprintln!(
            "ren-servo: saving the filters in {} failed: {err}",
            config.cache.display()
        );
    }
    Ok(engine)
}

fn read_cache(cache: &Path, key: &str) -> Option<Engine> {
    let data = fs::read(cache).ok()?;
    let rest = data
        .strip_prefix(key.as_bytes())?
        .strip_prefix(&[CACHE_SEPARATOR])?;
    let mut engine = Engine::default();
    // Another version of the engine may not read it; then it's made again.
    engine.deserialize(rest).ok()?;
    Some(engine)
}

fn write_cache(cache: &Path, key: &str, engine: &Engine) -> io::Result<()> {
    if let Some(dir) = cache.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut data = key.as_bytes().to_vec();
    data.push(CACHE_SEPARATOR);
    data.extend_from_slice(&engine.serialize());
    let temporary = cache.with_extension("tmp");
    fs::write(&temporary, data)?;
    fs::rename(&temporary, cache)
}

/// Makes an engine from the network rules of the lists that exist.
pub fn build_engine(lists: &[PathBuf]) -> Result<Engine, String> {
    let mut set = FilterSet::new(false);
    let mut any = false;
    for list in lists {
        let text = match read_list(list) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(format!("{}: {err}", list.display())),
        };
        set.add_filter_list(
            text,
            ParseOptions {
                rule_types: RuleTypes::NetworkOnly,
                ..ParseOptions::default()
            },
        );
        any = true;
    }
    if !any {
        return Err("no filter lists yet".to_owned());
    }
    Ok(Engine::new_with_filter_set(set))
}

fn read_list(path: &Path) -> io::Result<String> {
    use std::io::Read;
    let mut text = String::new();
    fs::File::open(path)?
        .take(MAX_LIST_BYTES)
        .read_to_string(&mut text)?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "\
! Title: test
||ads.example^
/banner/*$image
||tracker.example^$third-party
@@||ads.example/allowed.js
";

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    fn engine() -> Engine {
        let mut set = FilterSet::new(false);
        set.add_filter_list(LIST.to_owned(), ParseOptions::default());
        Engine::new_with_filter_set(set)
    }

    #[test]
    fn blocks_by_rules_and_exceptions() {
        let engine = engine();
        let page = url("https://news.example/article");
        let block =
            |u: &str, destination: &str| should_block(&engine, &url(u), Some(&page), destination);
        assert!(block("https://ads.example/a.js", "script"));
        assert!(!block("https://ads.example/allowed.js", "script"));
        assert!(block("https://cdn.example/banner/1.png", "image"));
        assert!(!block("https://cdn.example/banner/1.png", "script"));
        assert!(block("https://tracker.example/t.gif", "image"));
        assert!(!block("https://news.example/main.css", "style"));
        // First party for tracker.example's own pages.
        let own = url("https://tracker.example/");
        assert!(!should_block(
            &engine,
            &url("https://tracker.example/t.gif"),
            Some(&own),
            "image"
        ));
    }

    #[test]
    fn maps_destinations() {
        assert_eq!(request_type("script"), "script");
        assert_eq!(request_type("serviceworker"), "script");
        assert_eq!(request_type("style"), "stylesheet");
        assert_eq!(request_type("iframe"), "subdocument");
        assert_eq!(request_type("video"), "media");
        assert_eq!(request_type(""), "xmlhttprequest");
        assert_eq!(request_type("manifest"), "other");
    }

    #[test]
    fn unblocked_sites_cover_subdomains() {
        let sites = vec!["heise.de".to_owned(), "".to_owned()];
        assert!(is_unblocked(&sites, "heise.de"));
        assert!(is_unblocked(&sites, "www.heise.de"));
        assert!(is_unblocked(&sites, "WWW.Heise.DE."));
        assert!(!is_unblocked(&sites, "notheise.de"));
        assert!(!is_unblocked(&sites, "heise.de.example"));
        assert!(!is_unblocked(&sites, "example.org"));
        assert!(!is_unblocked(&[], "heise.de"));
    }

    fn temporary_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ren-servo-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn caches_the_engine_until_lists_change() {
        let dir = temporary_dir("filters");
        let list = dir.join("list.txt");
        let cache = dir.join("engine.dat");
        let config = Blocking {
            lists: vec![dir.join("missing.txt"), list.clone()],
            cache: cache.clone(),
            unblocked: Vec::new(),
        };
        assert!(load_engine(&config).is_err());
        assert!(!cache.exists());

        fs::write(&list, LIST).unwrap();
        let engine = load_engine(&config).unwrap();
        let ad = url("https://ads.example/a.js");
        assert!(should_block(&engine, &ad, None, "script"));
        let key = lists_key(&config.lists);
        assert!(read_cache(&cache, &key).is_some());
        assert!(!lists_changed(&config, &Some(config.clone())));

        // A new download: the key changes, and so does the engine.
        fs::write(&list, "||other.example^\n").unwrap();
        assert_ne!(lists_key(&config.lists), key);
        assert!(lists_changed(&config, &Some(config.clone())));
        let engine = load_engine(&config).unwrap();
        assert!(!should_block(&engine, &ad, None, "script"));
        assert!(should_block(
            &engine,
            &url("https://other.example/"),
            None,
            "image"
        ));

        // A broken cache is made again.
        fs::write(&cache, b"garbage").unwrap();
        assert!(load_engine(&config).is_ok());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn waits_for_the_engine_then_blocks() {
        let dir = temporary_dir("blocker");
        let list = dir.join("list.txt");
        fs::write(&list, LIST).unwrap();
        let config = Blocking {
            lists: vec![list],
            cache: dir.join("engine.dat"),
            unblocked: vec!["unblocked.example".to_owned()],
        };
        let (sender, receiver) = mpsc::channel();
        let waker: Waker = std::sync::Arc::new(move || {
            let _ = sender.send(());
        });
        let mut blocker = Blocker::default();
        let ad = url("https://ads.example/a.js");
        let page = url("https://news.example/");
        assert_eq!(
            blocker.check(&ad, Some(&page), None, "script"),
            Verdict::Allow
        );

        blocker.configure(Some(config.clone()), &waker);
        assert_eq!(
            blocker.check(&ad, Some(&page), None, "script"),
            Verdict::Wait
        );
        receiver.recv().unwrap();
        assert!(blocker.poll());
        assert_eq!(
            blocker.check(&ad, Some(&page), None, "script"),
            Verdict::Block
        );
        let unblocked = url("https://www.unblocked.example/");
        assert_eq!(
            blocker.check(&ad, Some(&page), Some(&unblocked), "script"),
            Verdict::Allow
        );
        assert_eq!(
            blocker.check(&ad, Some(&unblocked), Some(&page), "script"),
            Verdict::Allow
        );
        assert_eq!(blocker.take_counts(), (1, 1));

        // The same lists again: nothing to load.
        blocker.configure(Some(config), &waker);
        assert!(!blocker.poll());
        assert_eq!(
            blocker.check(&ad, Some(&page), None, "script"),
            Verdict::Block
        );

        blocker.configure(None, &waker);
        assert_eq!(
            blocker.check(&ad, Some(&page), None, "script"),
            Verdict::Allow
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
