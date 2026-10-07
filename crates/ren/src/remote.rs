// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Command line modes that talk to the server: `--check`, `--fetch-unread`
//! and `--dump-items` (spike S2), `--check-writes` (M1), `--sync` (M3).

use std::cell::Cell;
use std::io::Read;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nextcloud_news::types::{Feeds, Folder, Item, Items};
use nextcloud_news::{
    Client, Config, Credentials, Endpoint, Error, ItemAction, ItemQuery, Pager, ReadScope,
    Selection, Update,
};
use ren_settings::Settings;
use ren_store::Store;
use ren_sync::{NewsApi, Progress, SystemClock};

use crate::account;
use crate::cli::{Mode, Options};
use crate::procstat::{cpu_seconds, proc_status_kib, trim_heap};

pub const USER_AGENT: &str = concat!("ren/", env!("CARGO_PKG_VERSION"));

/// Runs one of the remote modes.
pub fn run(options: &Options) -> Result<(), String> {
    let settings = account::settings(options)?;
    let account = settings.account()?;
    let password = account::password(&account)?;
    let credentials = Credentials {
        user: account.user.clone(),
        password,
    };
    let client = Client::new(&account.server, &credentials, &Config::new(USER_AGENT));

    let result = match &options.mode {
        Mode::Window | Mode::SetPassword | Mode::CheckSecretService => {
            unreachable!("not a remote mode")
        }
        Mode::Check => check(&client),
        Mode::FetchUnread => fetch_unread(&client, options.batch_size),
        Mode::DumpItems(dir) => dump_items(&client, dir, options.batch_size),
        Mode::CheckWrites => check_writes(&client),
        Mode::Sync(db) => sync(&client, db, options, &settings),
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

/// Fetches all unread items page by page, decoding each item while it is
/// received and dropping it, like the sync will do after storing it.
/// Prints one line of measurements.
fn fetch_unread(client: &Client, batch_size: Option<u32>) -> CmdResult<()> {
    let rss_before = proc_status_kib("VmRSS:");
    let cpu_before = cpu_seconds();
    let started = Instant::now();
    let mut pager = Pager::new(ItemQuery::unread(batch_size));
    let (mut pages, mut items, mut repeated, mut bytes) = (0, 0, 0, 0);
    let mut body_bytes = 0;
    while let Some(query) = pager.start_page() {
        let mut reader = Counting {
            inner: client.get_reader(&Endpoint::Items(query))?,
            bytes: 0,
        };
        nextcloud_news::decode_items(&mut reader, |item| {
            if pager.accept(&item) {
                body_bytes += item.body.map_or(0, |body| body.len() as u64);
            }
            Ok::<_, nextcloud_news::Error>(())
        })?;
        bytes += reader.bytes;
        let info = pager.finish_page();
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
fn outside_source_tree(path: &Path) -> CmdResult<PathBuf> {
    let path = resolve(path)?;
    if source_tree().is_some_and(|tree| path.starts_with(tree)) {
        return Err(format!(
            "{} is inside the source tree; use a place outside the repository",
            path.display()
        )
        .into());
    }
    Ok(path)
}

fn prepare_dump_dir(dir: &Path) -> CmdResult<PathBuf> {
    let dir = outside_source_tree(dir)?;
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
        while let Some(query) = pager.start_page() {
            page_no += 1;
            let body = fetch(
                &Endpoint::Items(query),
                &format!("{name}-{page_no:04}.json"),
            )?;
            for item in serde_json::from_slice::<Items>(&body)?.items {
                if pager.accept(&item) {
                    newest_change = item.last_modified.max(newest_change);
                }
            }
            count += pager.finish_page().new;
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

/// An item as the server has it now: `GET /items` for its feed, starting
/// right above its id.
fn fetch_item(client: &Client, feed_id: u64, id: u64) -> CmdResult<Item> {
    let query = ItemQuery {
        selection: Selection::Feed(feed_id),
        get_read: true,
        // Two, in case the offset is inclusive and the feed has the next id.
        batch_size: Some(2),
        offset: id + 1,
    };
    let mut found = None;
    client.items(query, |item| {
        if item.id == id {
            found = Some(item);
        }
        Ok::<_, Error>(())
    })?;
    Ok(found.ok_or(format!("item {id} is gone"))?)
}

fn describe(unread: bool, starred: bool) -> String {
    format!(
        "{}, {}",
        if unread { "unread" } else { "read" },
        if starred { "starred" } else { "not starred" }
    )
}

/// Sends the write requests against the real server: the read and starred
/// state of the newest item is toggled and checked, "mark all as read" is
/// sent for items up to id 0, which matches none. The item's state is
/// restored at the end, also after a failure.
fn check_writes(client: &Client) -> CmdResult<()> {
    let newest = ItemQuery {
        selection: Selection::All,
        get_read: true,
        batch_size: Some(1),
        offset: 0,
    };
    let mut original = None;
    client.items(newest, |item| {
        original = Some(item);
        Ok::<_, Error>(())
    })?;
    let original = original.ok_or("there are no items on the server")?;
    println!(
        "Item {} of feed {}: {}",
        original.id,
        original.feed_id,
        describe(original.unread, original.starred)
    );

    let result = write_checks(client, &original);
    // Restore the state, whatever happened.
    let restore = |action| {
        client.update(&Update::Items {
            action,
            ids: &[original.id],
        })
    };
    restore(if original.unread {
        ItemAction::Unread
    } else {
        ItemAction::Read
    })?;
    restore(if original.starred {
        ItemAction::Star
    } else {
        ItemAction::Unstar
    })?;
    let item = fetch_item(client, original.feed_id, original.id)?;
    if (item.unread, item.starred) != (original.unread, original.starred) {
        return Err(format!(
            "item {} is {} instead of {}",
            item.id,
            describe(item.unread, item.starred),
            describe(original.unread, original.starred)
        )
        .into());
    }
    println!(
        "Restored item {}: {}",
        item.id,
        describe(item.unread, item.starred)
    );
    result
}

fn write_checks(client: &Client, original: &Item) -> CmdResult<()> {
    use ItemAction::{Read, Star, Unread, Unstar};

    let (unread, starred) = (original.unread, original.starred);
    let steps = [
        if unread { Read } else { Unread },
        if unread { Unread } else { Read },
        if starred { Unstar } else { Star },
        if starred { Star } else { Unstar },
    ];
    for action in steps {
        let update = Update::Items {
            action,
            ids: &[original.id],
        };
        client.update(&update)?;
        let expected = match action {
            Read => (false, starred),
            Unread => (true, starred),
            Star => (unread, true),
            Unstar => (unread, false),
        };
        let item = fetch_item(client, original.feed_id, original.id)?;
        let now = (item.unread, item.starred);
        if now != expected {
            return Err(format!(
                "POST {}: the item is {}, expected {}",
                update.path(),
                describe(now.0, now.1),
                describe(expected.0, expected.1)
            )
            .into());
        }
        println!("POST {}: ok, now {}", update.path(), describe(now.0, now.1));
    }

    let folder = client
        .feeds()?
        .feeds
        .iter()
        .find(|feed| feed.id == original.feed_id)
        .and_then(|feed| feed.folder_id)
        .filter(|&id| id != 0);
    let mut scopes = vec![ReadScope::All, ReadScope::Feed(original.feed_id)];
    scopes.extend(folder.map(ReadScope::Folder));
    for scope in scopes {
        let update = Update::MarkRead {
            scope,
            newest_item_id: 0,
        };
        client.update(&update)?;
        println!("POST {} up to item 0: ok", update.path());
    }
    // Whether feeds outside of folders can be marked as read as a folder.
    let update = Update::MarkRead {
        scope: ReadScope::Folder(0),
        newest_item_id: 0,
    };
    match client.update(&update) {
        Ok(()) => println!("POST {} up to item 0: ok", update.path()),
        Err(err) => println!("POST {} up to item 0: {err} (not used)", update.path()),
    }
    let item = fetch_item(client, original.feed_id, original.id)?;
    if (item.unread, item.starred) != (unread, starred) {
        return Err("marking all items up to id 0 as read changed the item".into());
    }
    Ok(())
}

/// The client, counting its requests.
struct CountingApi<'a> {
    client: &'a Client,
    requests: Cell<usize>,
}

impl CountingApi<'_> {
    fn count(&self) {
        self.requests.set(self.requests.get() + 1);
    }
}

impl NewsApi for CountingApi<'_> {
    fn folders(&self) -> Result<Vec<Folder>, Error> {
        self.count();
        self.client.folders()
    }

    fn feeds(&self) -> Result<Feeds, Error> {
        self.count();
        self.client.feeds()
    }

    fn items<E: From<Error>>(
        &self,
        query: ItemQuery,
        f: impl FnMut(Item) -> Result<(), E>,
    ) -> Result<usize, E> {
        self.count();
        self.client.items(query, f)
    }

    fn updated_items<E: From<Error>>(
        &self,
        last_modified: i64,
        f: impl FnMut(Item) -> Result<(), E>,
    ) -> Result<usize, E> {
        self.count();
        NewsApi::updated_items(self.client, last_modified, f)
    }

    fn update(&self, update: &Update<'_>) -> Result<(), Error> {
        self.count();
        self.client.update(update)
    }

    fn server_time(&self) -> Option<i64> {
        self.client.server_time()
    }
}

/// Syncs into the database `db` and prints one line of measurements. A
/// new database has no local changes, so this only reads from the server.
fn sync(client: &Client, db: &Path, options: &Options, settings: &Settings) -> CmdResult<()> {
    let batch_size = options
        .batch_size
        .ok_or("--sync pages through the items and needs a batch size")?;
    let db = outside_source_tree(db)?;
    if let Some(dir) = db.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let rss_before = proc_status_kib("VmRSS:");
    let cpu_before = cpu_seconds();
    let started = Instant::now();
    let mut store = Store::open(&db)?;
    let api = CountingApi {
        client,
        requests: Cell::new(0),
    };
    let sync_options = ren_sync::Options {
        batch_size,
        resync_after: if options.resync {
            Duration::ZERO
        } else {
            ren_sync::Options::default().resync_after
        },
        purge_after: settings.keep_read(),
        ..ren_sync::Options::default()
    };
    let mut next_note = 10_000;
    let report = ren_sync::sync(&api, &mut store, &SystemClock, &sync_options, |progress| {
        match progress {
            Progress::Feeds => eprintln!("folders and feeds stored"),
            Progress::Items { stored, expected } if stored >= next_note => {
                let of = expected.map_or(String::new(), |n| format!(" of about {n}"));
                eprintln!("{stored} items stored{of}");
                next_note += 10_000;
            }
            _ => {}
        }
        ControlFlow::Continue(())
    })?;
    let seconds = started.elapsed().as_secs_f64();
    let cpu = cpu_seconds()
        .zip(cpu_before)
        .map(|((user, sys), (user0, sys0))| (user - user0, sys - sys0));
    let peak = proc_status_kib("VmHWM:");
    let rss_after = proc_status_kib("VmRSS:");
    let rss_trimmed = if trim_heap() {
        proc_status_kib("VmRSS:")
    } else {
        None
    };
    let mut wal = db.clone().into_os_string();
    wal.push("-wal");
    let db_bytes = [db.as_os_str(), &wal]
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|meta| meta.len())
        .sum::<u64>();

    if let Some(err) = &report.fallback {
        eprintln!("fetching the changes failed, resynced instead: {err}");
    }
    for err in &report.push_errors {
        eprintln!("the server refused a change: {err}");
    }
    let kind = match report.kind {
        ren_sync::Kind::Initial => "initial",
        ren_sync::Kind::Incremental => "incremental",
        ren_sync::Kind::Resync => "resync",
    };
    let or_unknown = |v: Option<String>| v.unwrap_or_else(|| "?".to_owned());
    let kib = |v: Option<u64>| or_unknown(v.map(|k| k.to_string()));
    println!(
        "sync: kind={kind} resumed={} requests={} items={} added={} reconciled={} purged={} \
         pushed={} marked_read={} push_errors={} seconds={seconds:.3} user_seconds={} \
         sys_seconds={} rss_before_kib={} peak_rss_kib={} rss_after_kib={} \
         rss_trimmed_kib={} db_bytes={db_bytes}",
        report.resumed,
        api.requests.get(),
        report.items,
        report.added,
        report.reconciled,
        report.purged,
        report.pushed,
        report.marked_read,
        report.push_errors.len(),
        or_unknown(cpu.map(|(user, _)| format!("{user:.2}"))),
        or_unknown(cpu.map(|(_, sys)| format!("{sys:.2}"))),
        kib(rss_before),
        kib(peak),
        kib(rss_after),
        kib(rss_trimmed),
    );
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
