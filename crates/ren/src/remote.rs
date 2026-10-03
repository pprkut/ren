// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Command line modes that talk to the server: `--check`, `--fetch-unread`
//! and `--dump-items` (spike S2).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use nextcloud_news::types::Items;
use nextcloud_news::{Client, Credentials, Endpoint, ItemQuery, Pager, Selection};

use crate::cli::{Mode, Options};
use crate::procstat::{cpu_seconds, proc_status_kib};
use crate::settings;

const USER_AGENT: &str = concat!("ren/", env!("CARGO_PKG_VERSION"));

/// Runs one of the remote modes.
pub fn run(options: &Options) -> Result<(), String> {
    let path = match &options.settings {
        Some(path) => path.clone(),
        None => settings::default_path(|name| std::env::var(name).ok())
            .ok_or("cannot find the settings file: neither XDG_CONFIG_HOME nor HOME is set")?,
    };
    let account = settings::load_account(&path, options.settings.is_some())?;
    let password = settings::password(&account, std::env::var(settings::PASSWORD_VAR).ok())?;
    let credentials = Credentials {
        user: account.user.clone(),
        password,
    };
    let client = Client::new(&account.server, &credentials, USER_AGENT);

    let result = match &options.mode {
        Mode::Window => unreachable!("the window is not a remote mode"),
        Mode::Check => check(&client),
        Mode::FetchUnread => fetch_unread(&client, options.batch_size),
        Mode::DumpItems(dir) => dump_items(&client, dir, options.batch_size),
    };
    result.map_err(|err| format!("{}: {err}", client.base_url()))
}

type CmdResult<T> = Result<T, Box<dyn std::error::Error>>;

fn check(client: &Client) -> CmdResult<()> {
    let status = client.status()?;
    let folders = client.folders()?;
    let feeds = client.feeds()?;

    println!("Server: {}", client.base_url());
    println!("News app version: {}", status.version);
    if status.warnings.improperly_configured_cron {
        println!("Warning: the News app updater is improperly configured; feeds are not updated");
    }
    if status.warnings.incorrect_db_charset {
        println!("Warning: the database charset is set up incorrectly");
    }
    println!("Folders: {}", folders.len());
    let failing = feeds
        .feeds
        .iter()
        .filter(|f| f.update_error_count > 0)
        .count();
    println!(
        "Feeds: {} ({failing} with update errors)",
        feeds.feeds.len()
    );
    let unread: u64 = feeds.feeds.iter().map(|f| f.unread_count).sum();
    println!("Unread items: {unread}");
    println!("Starred items: {}", feeds.starred_count);
    if let Some(id) = feeds.newest_item_id {
        println!("Newest item id: {id}");
    }
    Ok(())
}

/// Counts the bytes read through it.
struct Counting<R> {
    inner: R,
    bytes: u64,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes += n as u64;
        Ok(n)
    }
}

/// Fetches all unread items page by page, decoding each page while it is
/// received and dropping it, like the sync will do before storing it.
/// Prints one line of measurements.
fn fetch_unread(client: &Client, batch_size: Option<u32>) -> CmdResult<()> {
    let rss_before = proc_status_kib("VmRSS:");
    let cpu_before = cpu_seconds();
    let started = Instant::now();
    let mut pager = Pager::new(ItemQuery::unread(batch_size));
    let (mut pages, mut items, mut repeated, mut bytes) = (0, 0, 0, 0);
    let mut body_bytes = 0;
    while let Some(query) = pager.next_query() {
        let mut reader = Counting {
            inner: client.get_reader(&Endpoint::Items(query))?,
            bytes: 0,
        };
        let mut page = nextcloud_news::decode::<Items>(&mut reader)?.items;
        bytes += reader.bytes;
        let info = pager.advance(&mut page);
        body_bytes += page
            .iter()
            .filter_map(|item| item.body.as_ref())
            .map(|body| body.len() as u64)
            .sum::<u64>();
        pages += 1;
        items += info.new;
        repeated += info.repeated;
    }
    let seconds = started.elapsed().as_secs_f64();
    let cpu = cpu_seconds()
        .zip(cpu_before)
        .map(|((user, sys), (user0, sys0))| (user - user0, sys - sys0));

    let batch = batch_size.map_or("all".to_owned(), |n| n.to_string());
    let or_unknown = |v: Option<String>| v.unwrap_or_else(|| "?".to_owned());
    println!(
        "fetch: batch_size={batch} pages={pages} items={items} repeated={repeated} \
         json_bytes={bytes} body_bytes={body_bytes} seconds={seconds:.3} \
         user_seconds={} sys_seconds={} rss_before_kib={} peak_rss_kib={}",
        or_unknown(cpu.map(|(user, _)| format!("{user:.2}"))),
        or_unknown(cpu.map(|(_, sys)| format!("{sys:.2}"))),
        or_unknown(rss_before.map(|k| k.to_string())),
        or_unknown(proc_status_kib("VmHWM:").map(|k| k.to_string())),
    );
    Ok(())
}

/// The source tree this binary was built from; dumps must not go there.
fn source_tree() -> Option<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)?
        .canonicalize()
        .ok()
}

/// `path` made absolute with symlinks resolved, also if it doesn't exist
/// yet: the longest existing ancestor is canonicalised.
fn resolve(path: &Path) -> std::io::Result<PathBuf> {
    let path = std::path::absolute(path)?;
    let mut existing = path.as_path();
    let mut rest = Vec::new();
    while !existing.exists() {
        let Some(parent) = existing.parent() else {
            break;
        };
        rest.extend(existing.file_name());
        existing = parent;
    }
    let mut resolved = existing.canonicalize()?;
    resolved.extend(rest.iter().rev());
    Ok(resolved)
}

/// Real feed data must not end up in the repository.
fn prepare_dump_dir(dir: &Path) -> CmdResult<PathBuf> {
    let dir = resolve(dir)?;
    if source_tree().is_some_and(|tree| dir.starts_with(tree)) {
        return Err(format!(
            "{} is inside the source tree; dump outside the repository",
            dir.display()
        )
        .into());
    }
    std::fs::create_dir_all(&dir)?;
    if std::fs::read_dir(&dir)?.next().is_some() {
        return Err(format!("{} is not empty", dir.display()).into());
    }
    Ok(dir)
}

/// Stores the raw responses for later spikes and test fixtures.
fn dump_items(client: &Client, dir: &Path, batch_size: Option<u32>) -> CmdResult<()> {
    let dir = prepare_dump_dir(dir)?;
    let fetch = |endpoint: &Endpoint, name: &str| -> CmdResult<Vec<u8>> {
        let mut body = Vec::new();
        client.get_reader(endpoint)?.read_to_end(&mut body)?;
        std::fs::write(dir.join(name), &body)?;
        Ok(body)
    };

    fetch(&Endpoint::Version, "version.json")?;
    fetch(&Endpoint::Status, "status.json")?;
    fetch(&Endpoint::Folders, "folders.json")?;
    fetch(&Endpoint::Feeds, "feeds.json")?;

    let mut newest_change = None;
    for (name, query) in [
        ("unread", ItemQuery::unread(batch_size)),
        ("starred", ItemQuery::starred(batch_size)),
    ] {
        let mut pager = Pager::new(query);
        let (mut page_no, mut count) = (0, 0);
        while let Some(query) = pager.next_query() {
            page_no += 1;
            let body = fetch(
                &Endpoint::Items(query),
                &format!("{name}-{page_no:04}.json"),
            )?;
            let mut page = serde_json::from_slice::<Items>(&body)?.items;
            newest_change = page
                .iter()
                .filter_map(|item| item.last_modified)
                .chain(newest_change)
                .max();
            count += pager.advance(&mut page).new;
        }
        eprintln!("{name}: {count} items in {page_no} pages");
    }

    // A sample of /items/updated: everything changed in the last day before
    // the newest change.
    if let Some(newest) = newest_change {
        let endpoint = Endpoint::UpdatedItems {
            last_modified: newest - 86_400,
            selection: Selection::All,
        };
        let body = fetch(&endpoint, "updated-1d.json")?;
        let count = serde_json::from_slice::<Items>(&body)?.items.len();
        eprintln!("updated in the last day: {count} items");
    }
    eprintln!("raw responses written to {}", dir.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dump_dir_inside_source_tree_is_refused() {
        let tree = source_tree().unwrap();
        let dir = tree.join("target").join("dump-test").join("sub");
        let err = prepare_dump_dir(&dir).unwrap_err();
        assert!(err.to_string().contains("source tree"));
        assert!(!dir.exists());
    }

    #[test]
    fn resolve_missing_path() {
        let tree = source_tree().unwrap();
        let missing = tree.join("crates").join("..").join("no-such-dir").join("x");
        assert_eq!(
            resolve(&missing).unwrap(),
            tree.join("no-such-dir").join("x")
        );
    }

    #[test]
    fn counting_reader() {
        let mut reader = Counting {
            inner: &b"hello"[..],
            bytes: 0,
        };
        let mut out = String::new();
        reader.read_to_string(&mut out).unwrap();
        assert_eq!(reader.bytes, 5);
    }
}
