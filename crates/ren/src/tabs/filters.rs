// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The filter lists for content blocking in the tabs: ren downloads them,
//! the helper makes its engine from them. See
//! `docs/decisions/0012-content-blocking.md`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use ren_tabs::Blocking;

use crate::hash::fnv1a;

/// How old a list may get before it's downloaded again: EasyList's
/// expiry (the lists ask for 1 to 7 days).
pub const MAX_AGE: Duration = Duration::from_secs(4 * 24 * 60 * 60);

/// The largest list, with its includes; the helper reads no more.
const MAX_LIST_BYTES: u64 = 16 * 1024 * 1024;

/// The most files a list may include.
const MAX_INCLUDES: usize = 32;

/// One update at a time, even if helpers come and go faster.
static UPDATING: AtomicBool = AtomicBool::new(false);

/// The directory the lists are kept in.
#[derive(Debug, Clone)]
pub struct Filters {
    dir: PathBuf,
}

impl Filters {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Where the list from `url` is kept.
    pub fn path(&self, url: &str) -> PathBuf {
        self.dir.join(format!("{:016x}.txt", fnv1a(url)))
    }

    /// What the helper needs to block with the lists from `urls`; none if
    /// there are none (blocking is off).
    pub fn blocking(&self, urls: &[String], unblocked: &[String]) -> Option<Blocking> {
        if urls.is_empty() {
            return None;
        }
        Some(Blocking {
            lists: urls.iter().map(|url| self.path(url)).collect(),
            cache: self.dir.join("engine.dat"),
            unblocked: unblocked.to_vec(),
        })
    }

    /// The lists of `urls` that are missing or older than [`MAX_AGE`].
    pub fn due(&self, urls: &[String], now: SystemTime) -> Vec<String> {
        urls.iter()
            .filter(|url| {
                let modified = fs::metadata(self.path(url)).and_then(|meta| meta.modified());
                match modified {
                    Ok(modified) => now.duration_since(modified).is_ok_and(|age| age >= MAX_AGE),
                    Err(_) => true,
                }
            })
            .cloned()
            .collect()
    }

    /// Downloads `urls` one after another on a thread of low priority;
    /// `done` is called there with whether any list was written. Returns
    /// false, without downloading, if an update is running already.
    pub fn update(&self, urls: Vec<String>, done: impl FnOnce(bool) + Send + 'static) -> bool {
        if urls.is_empty() || UPDATING.swap(true, Ordering::AcqRel) {
            return false;
        }
        let filters = self.clone();
        let spawned = std::thread::Builder::new()
            .name("ren-filters".to_owned())
            .spawn(move || {
                crate::sync_thread::lower_priority();
                let agent = crate::http::agent();
                let mut changed = false;
                for url in &urls {
                    match filters.download(&agent, url) {
                        Ok(()) => changed = true,
                        Err(err) => eprintln!("ren: downloading the filter list {url}: {err}"),
                    }
                }
                UPDATING.store(false, Ordering::Release);
                done(changed);
            });
        if spawned.is_err() {
            UPDATING.store(false, Ordering::Release);
            return false;
        }
        true
    }

    /// Downloads the list from `url` and its includes, and writes it as
    /// the engine reads it. A failure keeps the old list.
    fn download(&self, agent: &ureq::Agent, url: &str) -> Result<(), String> {
        let fetch = |url: &str| -> Result<String, String> {
            let bytes = crate::http::fetch(agent, url, MAX_LIST_BYTES)?;
            String::from_utf8(bytes).map_err(|_| "not UTF-8".to_owned())
        };
        let text = fetch(url)?;
        let mut included = 0;
        let mut failed = None;
        let list = prepare(&text, &mut |name| {
            let Some(include) = include_url(url, name) else {
                eprintln!("ren: {url} includes {name}, which isn't next to it");
                return None;
            };
            included += 1;
            if included > MAX_INCLUDES {
                failed = Some(format!("more than {MAX_INCLUDES} includes"));
                return None;
            }
            fetch(&include)
                .inspect_err(|err| failed = Some(format!("{include}: {err}")))
                .ok()
        });
        // Half a list could unblock what the old one blocks.
        if let Some(err) = failed {
            return Err(err);
        }
        if list.len() as u64 > MAX_LIST_BYTES {
            return Err("too large with its includes".to_owned());
        }
        write_atomically(&self.path(url), list.as_bytes()).map_err(|err| err.to_string())
    }
}

/// Writes a new file and renames it over the old one, so the helper never
/// reads half a list.
fn write_atomically(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let temporary = path.with_extension("part");
    fs::write(&temporary, data)?;
    fs::rename(&temporary, path)
}

/// The URL of a file a list at `base` includes: only a file name, which
/// is looked for next to the list, as uBlock Origin does.
pub fn include_url(base: &str, name: &str) -> Option<String> {
    let plain = !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    if !plain {
        return None;
    }
    let dir = &base[..=base.rfind('/')?];
    Some(format!("{dir}{name}"))
}

/// Fetches a file a list includes, by its name.
pub type Include<'a> = dyn FnMut(&str) -> Option<String> + 'a;

/// A list as the adblock engine reads it: uBlock Origin's `!#include`
/// lines replaced by the files they name (from `include`, one level deep)
/// and its `!#if` … `!#else` … `!#endif` sections evaluated; the engine
/// reads either as comments.
pub fn prepare(text: &str, include: &mut Include) -> String {
    let mut out = String::with_capacity(text.len());
    add_lines(text, &mut out, Some(include));
    out
}

fn add_lines(
    text: &str,
    out: &mut String,
    mut include: Option<&mut Include>,
) {
    // Whether each open `!#if` section is taken.
    let mut sections: Vec<bool> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_end();
        if let Some(condition) = trimmed.strip_prefix("!#if ") {
            sections.push(evaluate(condition));
            continue;
        }
        if trimmed == "!#else" {
            if let Some(taken) = sections.last_mut() {
                *taken = !*taken;
            }
            continue;
        }
        if trimmed == "!#endif" {
            sections.pop();
            continue;
        }
        if !sections.iter().all(|&taken| taken) {
            continue;
        }
        if let Some(name) = trimmed.strip_prefix("!#include ") {
            // Included files can't include others.
            if let Some(include) = include.as_mut()
                && let Some(text) = include(name.trim())
            {
                add_lines(&text, out, None);
            }
            continue;
        }
        out.push_str(trimmed);
        out.push('\n');
    }
}

/// Evaluates an `!#if` condition: tokens with `!`, `&&`, `||` and
/// parentheses. ren is uBlock-compatible (`ext_ublock`) and none of the
/// browsers or platforms the lists distinguish; unknown tokens are
/// false.
pub fn evaluate(condition: &str) -> bool {
    let tokens = tokenize(condition);
    let mut parser = Parser {
        tokens: &tokens,
        next: 0,
    };
    let value = parser.or();
    // Something it can't read is taken as false.
    value && parser.next == tokens.len()
}

fn tokenize(condition: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut rest = condition.trim();
    while !rest.is_empty() {
        let len = if rest.starts_with("&&") || rest.starts_with("||") {
            2
        } else if rest.starts_with(['!', '(', ')']) {
            1
        } else {
            rest.find(|c: char| c.is_whitespace() || "!()&|".contains(c))
                .unwrap_or(rest.len())
                .max(1)
        };
        tokens.push(&rest[..len]);
        rest = rest[len..].trim_start();
    }
    tokens
}

struct Parser<'a> {
    tokens: &'a [&'a str],
    next: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&'a str> {
        self.tokens.get(self.next).copied()
    }

    fn or(&mut self) -> bool {
        let mut value = self.and();
        while self.peek() == Some("||") {
            self.next += 1;
            value |= self.and();
        }
        value
    }

    fn and(&mut self) -> bool {
        let mut value = self.not();
        while self.peek() == Some("&&") {
            self.next += 1;
            value &= self.not();
        }
        value
    }

    fn not(&mut self) -> bool {
        match self.peek() {
            Some("!") => {
                self.next += 1;
                !self.not()
            }
            Some("(") => {
                self.next += 1;
                let value = self.or();
                if self.peek() == Some(")") {
                    self.next += 1;
                }
                value
            }
            Some(token) => {
                self.next += 1;
                token == "ext_ublock"
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions() {
        assert!(evaluate("ext_ublock"));
        assert!(!evaluate("env_mobile"));
        assert!(evaluate("!env_mobile"));
        assert!(evaluate("!env_firefox && !env_chromium"));
        assert!(!evaluate("env_firefox || env_chromium"));
        assert!(evaluate("env_safari || ext_ublock"));
        assert!(!evaluate("ext_ublock && false"));
        assert!(evaluate("!(env_firefox || env_mobile)"));
        assert!(evaluate("!!ext_ublock"));
        assert!(!evaluate("ext_ublock )"));
        assert!(!evaluate(""));
    }

    #[test]
    fn includes_and_sections() {
        let list = "\
! Title: test
||a.example^
!#include more.txt
!#if env_mobile
||mobile.example^
!#include mobile.txt
!#else
||desktop.example^
!#if !ext_ubol
||not-lite.example^
!#endif
!#endif
!#include missing.txt
||b.example^
";
        let mut asked = Vec::new();
        let prepared = prepare(list, &mut |name| {
            asked.push(name.to_owned());
            match name {
                "more.txt" => Some("||more.example^\n!#include nested.txt\n".to_owned()),
                _ => None,
            }
        });
        assert_eq!(asked, ["more.txt", "missing.txt"]);
        assert_eq!(
            prepared,
            "! Title: test\n||a.example^\n||more.example^\n||desktop.example^\n\
             ||not-lite.example^\n||b.example^\n"
        );
    }

    #[test]
    fn includes_only_next_to_the_list() {
        let base = "https://lists.example/filters/filters.txt";
        assert_eq!(
            include_url(base, "filters-2024.txt").as_deref(),
            Some("https://lists.example/filters/filters-2024.txt")
        );
        assert_eq!(include_url(base, "../secret.txt"), None);
        assert_eq!(include_url(base, "sub/list.txt"), None);
        assert_eq!(include_url(base, "https://other.example/x.txt"), None);
        assert_eq!(include_url(base, ".hidden"), None);
        assert_eq!(include_url(base, ""), None);
    }

    #[test]
    fn paths_and_due_lists() {
        let dir = std::env::temp_dir().join(format!("ren-filters-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let filters = Filters::new(dir.clone());
        let urls = vec![
            "https://lists.example/a.txt".to_owned(),
            "https://lists.example/b.txt".to_owned(),
        ];
        assert_ne!(filters.path(&urls[0]), filters.path(&urls[1]));
        assert_eq!(filters.blocking(&[], &[]), None);
        let blocking = filters
            .blocking(&urls, &["example.org".to_owned()])
            .unwrap();
        assert_eq!(blocking.lists[0], filters.path(&urls[0]));
        assert_eq!(blocking.cache, dir.join("engine.dat"));
        assert_eq!(blocking.unblocked, ["example.org"]);

        let now = SystemTime::now();
        assert_eq!(filters.due(&urls, now), urls);
        write_atomically(&filters.path(&urls[0]), b"||a.example^\n").unwrap();
        assert_eq!(filters.due(&urls, now), urls[1..]);
        assert_eq!(filters.due(&urls, now + MAX_AGE), urls);
        fs::remove_dir_all(&dir).unwrap();
    }
}
