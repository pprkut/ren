// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Slint glue: binds the view models to the main window.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ren_settings::{Reload, Settings, SettingsFile};
use ren_store::{Status, Store};
use ren_sync::Progress;
use slint::language::ColorScheme;
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, VecModel};

use crate::cli::{self, Options};
use crate::demo::DemoDb;
use crate::feed_tree::{self, Node, Row, changed_rows};
use crate::item_list::{self, Column, format_date};
use crate::reader::{Changes, Reader};
use crate::sync_thread::{self, Event, Running};

mod article;
mod icons;
#[cfg(feature = "servo")]
mod pages;

slint::include_modules!();

/// The renderer with the smallest footprint, see
/// `docs/decisions/0001-ui-toolkit.md`.
const DEFAULT_RENDERER: &str = "software";

/// How long a message stays in the status bar.
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

/// Scroll speed of `--autoscroll`, in logical pixels per second.
const AUTOSCROLL_SPEED: f32 = 8_000.0;
/// Time between startup and the start of `--autoscroll`.
const AUTOSCROLL_DELAY: Duration = Duration::from_secs(3);

/// How often the lists are read again while a full sync stores items, so
/// that they fill up during the first sync.
const SYNC_REFRESH: Duration = Duration::from_secs(2);

/// Item list model that fetches each row from the store only when the
/// list view asks for it, so only the visible rows ever exist as Slint
/// values.
struct ItemListModel {
    reader: Rc<RefCell<Reader>>,
    notify: ModelNotify,
}

impl Model for ItemListModel {
    type Data = ItemRow;

    fn row_count(&self) -> usize {
        self.reader.borrow().len()
    }

    fn row_data(&self, row: usize) -> Option<ItemRow> {
        if row >= self.row_count() {
            return None;
        }
        let reader = self.reader.borrow();
        // An item deleted since the list was loaded shows as an empty row
        // until the list is loaded again.
        let item = match reader.row(row) {
            Ok(Some(item)) => item,
            Ok(None) => return Some(ItemRow::default()),
            Err(err) => {
                eprintln!("ren: reading row {row}: {err}");
                return Some(ItemRow::default());
            }
        };
        Some(ItemRow {
            title: item.title.into(),
            feed: reader.feed_title(item.feed_id).into(),
            author: item.author.into(),
            date: format_date(item.pub_date).into(),
            status: match item.status {
                Status::New => ItemStatus::New,
                Status::Unread => ItemStatus::Unread,
                Status::Read => ItemStatus::Read,
            },
            starred: item.starred,
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

fn feed_row(row: &Row) -> FeedRow {
    FeedRow {
        title: row.title.as_str().into(),
        depth: row.depth.into(),
        count: row.count.try_into().unwrap_or(i32::MAX),
        starred: row.node == Node::Starred,
        is_folder: row.expanded.is_some(),
        expanded: row.expanded == Some(true),
        last: row.last,
        guides: ModelRc::new(VecModel::from(row.guides.clone())),
    }
}

struct App {
    window: slint::Weak<MainWindow>,
    reader: Rc<RefCell<Reader>>,
    /// The rows the feed tree shows, to update only those that changed.
    tree_rows: RefCell<Vec<Row>>,
    feeds: Rc<VecModel<FeedRow>>,
    items: Rc<ItemListModel>,
    article: Rc<RefCell<article::ArticlePane>>,
    #[cfg(feature = "servo")]
    pages: Rc<RefCell<pages::Pages>>,
    /// Clears the status bar message after `STATUS_TIMEOUT`.
    status_timer: slint::Timer,
    /// The database to sync; `None` for demo data, which isn't synced.
    database: Option<PathBuf>,
    /// The settings file; `None` for demo data.
    settings: RefCell<Option<SettingsFile>>,
    sync: RefCell<SyncState>,
    /// Print what syncs take.
    measure: bool,
}

#[derive(Default)]
struct SyncState {
    running: Option<Running>,
    /// The account the running sync is for.
    account: Option<ren_settings::Account>,
    /// The app password from the last sync, with the account it is for.
    password: Option<(ren_settings::Account, String)>,
    /// When the lists were last read again during a sync.
    refreshed: Option<Instant>,
}

impl App {
    fn window(&self) -> MainWindow {
        self.window
            .upgrade()
            .expect("callbacks only run while the window exists")
    }

    /// Runs `f` on the reader; a database error goes to the status bar.
    fn with_reader<T>(&self, f: impl FnOnce(&mut Reader) -> ren_store::Result<T>) -> Option<T> {
        let result = f(&mut self.reader.borrow_mut());
        result
            .map_err(|err| self.status(format!("Database error: {err}")))
            .ok()
    }

    /// Updates the feed tree: only the rows that changed, unless rows were
    /// added or removed.
    fn refresh_tree(&self) {
        let (rows, selected, unread) = {
            let reader = self.reader.borrow();
            (reader.tree_rows(), reader.selected(), reader.unread())
        };
        let changed = changed_rows(&self.tree_rows.borrow(), &rows);
        match changed {
            Some(changed) => {
                for i in changed {
                    self.feeds.set_row_data(i, feed_row(&rows[i]));
                }
            }
            None => self
                .feeds
                .set_vec(rows.iter().map(feed_row).collect::<Vec<_>>()),
        }
        let current = rows.iter().position(|r| r.node == selected);
        *self.tree_rows.borrow_mut() = rows;
        let window = self.window();
        window.set_current_feed(current.map_or(-1, |i| i as i32));
        window.set_unread_text(format!("{unread} unread articles").into());
    }

    /// Updates what an operation of the reader changed.
    fn apply(&self, changes: Changes) {
        if changes.counts {
            self.refresh_tree();
        }
        if changes.list {
            self.items.notify.reset();
            self.keep_current_item();
        } else {
            for row in changes.rows {
                self.items.notify.row_changed(row);
            }
        }
        self.update_current_starred();
    }

    fn update_current_starred(&self) {
        let current = self.reader.borrow().current();
        let starred = current
            .and_then(|id| self.with_reader(|reader| reader.is_starred(id)))
            .unwrap_or(false);
        self.window().set_current_starred(starred);
    }

    /// Shows a message in the status bar for a few seconds.
    fn status(&self, text: impl Into<SharedString>) {
        self.window().set_status_text(text.into());
        let weak = self.window.clone();
        self.status_timer
            .start(slint::TimerMode::SingleShot, STATUS_TIMEOUT, move || {
                if let Some(window) = weak.upgrade() {
                    window.set_status_text(SharedString::new());
                }
            });
    }

    /// Shows a message in the status bar until the next one.
    fn status_until_next(&self, text: impl Into<SharedString>) {
        self.status_timer.stop();
        self.window().set_status_text(text.into());
    }

    fn not_implemented(&self, action: &str) {
        self.status(format!("Not implemented yet: {}", action.replace('-', " ")));
    }

    /// Marks an item read or unread.
    fn set_read(&self, id: u64, read: bool) {
        if let Some(changes) = self.with_reader(|reader| reader.set_read(id, read)) {
            self.apply(changes);
        }
    }

    fn toggle_star(&self, id: u64) {
        if let Some(changes) = self.with_reader(|reader| reader.toggle_star(id)) {
            self.apply(changes);
        }
    }

    /// Marks all items of a node read.
    fn mark_node_read(&self, node: Node) {
        if let Some((count, changes)) = self.with_reader(|reader| reader.mark_all_read(node)) {
            self.apply(changes);
            self.status(format!("Marked {count} articles as read"));
        }
    }

    /// Selects the next unread item after the current one.
    fn next_unread(&self) {
        match self.with_reader(|reader| reader.next_unread()) {
            Some(Some(row)) => self.item_clicked(row),
            Some(None) => self.status("No more unread articles"),
            None => {}
        }
    }

    fn action(&self, name: &str) {
        let current = self.reader.borrow().current();
        match name {
            "quit" => {
                let _ = slint::quit_event_loop();
            }
            "sort-title" => self.sort_by(Column::Title),
            "sort-feed" => self.sort_by(Column::Feed),
            "sort-author" => self.sort_by(Column::Author),
            "sort-date" => self.sort_by(Column::Date),
            "previous-article" => self.item_key("up", 1),
            "next-article" => self.item_key("down", 1),
            "next-unread" => self.next_unread(),
            "mark-feed-read" => self.mark_node_read(self.reader.borrow().selected()),
            "mark-read" | "mark-unread" => {
                if let Some(id) = current {
                    self.set_read(id, name == "mark-read");
                }
            }
            "toggle-star" => {
                if let Some(id) = current {
                    self.toggle_star(id);
                }
            }
            "open-tab" | "open-browser" => {
                if let Some(id) = current {
                    self.open_item(id, name == "open-browser");
                }
            }
            "fetch-feed" | "fetch-all" => self.start_sync(),
            "cancel-sync" => self.cancel_sync(),
            "about" => self.status("ren: a native Nextcloud News reader"),
            _ => self.not_implemented(name),
        }
    }

    fn feed_menu(&self, row: usize, action: &str) {
        match (action, self.node(row)) {
            ("mark-feed-read", Some(node)) => self.mark_node_read(node),
            // The server fetches the feeds; ren syncs everything.
            ("fetch-feed" | "fetch-all", _) => self.start_sync(),
            _ => self.not_implemented(action),
        }
    }

    fn item_menu(&self, row: usize, action: &str) {
        let Some(id) = self.reader.borrow().id(row) else {
            return;
        };
        match action {
            "mark-read" | "mark-unread" => self.set_read(id, action == "mark-read"),
            "toggle-star" => self.toggle_star(id),
            "open-tab" | "open-browser" => self.open_item(id, action == "open-browser"),
            _ => self.not_implemented(action),
        }
    }

    /// Opens the item's web page in a tab, or in the system browser.
    fn open_item(&self, id: u64, browser: bool) {
        let Some(url) = self.with_reader(|reader| reader.url(id)) else {
            return;
        };
        let Some(url) = url else {
            self.status("This article has no link");
            return;
        };
        if browser {
            self.open_in_browser(&url);
        } else {
            self.open_url(&url);
        }
    }

    /// Opens a web page in a tab (without Servo in the system browser), a
    /// mail link in the desktop's mail client.
    fn open_url(&self, url: &str) {
        match link_kind(url) {
            #[cfg(feature = "servo")]
            LinkKind::Web => {
                if let Err(err) = self.pages.borrow_mut().open(url) {
                    self.status(format!("Opening the page failed: {err}"));
                }
            }
            _ => self.open_in_browser(url),
        }
    }

    /// Opens `url` with the desktop's default application (`xdg-open`).
    /// Only web and mail links: feed content mustn't open local files or
    /// start other applications through their URL schemes.
    fn open_in_browser(&self, url: &str) {
        if link_kind(url) == LinkKind::Other {
            self.status(format!("Not opening this kind of link: {url}"));
            return;
        }
        match std::process::Command::new("xdg-open")
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                // xdg-open returns soon; wait for it so it doesn't linger as
                // a zombie.
                std::thread::spawn(move || child.wait());
            }
            Err(err) => self.status(format!("Opening the browser failed: {err}")),
        }
    }

    /// A link in the article: opened in a tab, in the browser, or copied.
    fn article_link(&self, url: &str, action: &str) {
        match action {
            "open" => self.open_url(url),
            "browser" => self.open_in_browser(url),
            "copy" => self.article.borrow().copy_text(url),
            _ => self.not_implemented(action),
        }
    }

    fn search_edited(&self, text: &str) {
        if self.with_reader(|reader| reader.search(text)).is_some() {
            self.items.notify.reset();
            self.keep_current_item();
        }
    }

    /// Selects the current item's row again after the list changed, or
    /// nothing if it is no longer shown.
    fn keep_current_item(&self) {
        let row = self.reader.borrow().current_row();
        self.window().set_current_item(row.map_or(-1, |r| r as i32));
    }

    fn node(&self, row: usize) -> Option<Node> {
        self.tree_rows.borrow().get(row).map(|r| r.node)
    }

    fn feed_clicked(&self, row: usize) {
        if let Some(node) = self.node(row) {
            self.select_node(node);
        }
    }

    fn feed_toggled(&self, row: usize) {
        if let Some(Node::Folder(id)) = self.node(row) {
            self.reader.borrow_mut().toggle_folder(id);
        }
        self.refresh_tree();
    }

    fn feed_key(&self, key: &str) {
        let key = match key {
            "up" => feed_tree::Key::Up,
            "down" => feed_tree::Key::Down,
            "left" => feed_tree::Key::Left,
            "right" => feed_tree::Key::Right,
            "home" => feed_tree::Key::Home,
            "end" => feed_tree::Key::End,
            _ => return,
        };
        let next = self.reader.borrow_mut().navigate(key);
        match next {
            Some(node) => self.select_node(node),
            // Expanding or collapsing a folder.
            None => self.refresh_tree(),
        }
    }

    /// Selects a feed, a folder, "All items" or "Starred" and shows its
    /// items.
    fn select_node(&self, node: Node) {
        self.with_reader(|reader| reader.select(node));
        self.refresh_tree();
        self.items.notify.reset();
        let window = self.window();
        window.set_item_list_content_y(0.0);
        window.set_current_item(-1);
        window.set_current_starred(false);
        self.article.borrow_mut().clear();
    }

    fn sort_clicked(&self, column: usize) {
        if let Some(column) = Column::from_index(column) {
            self.sort_by(column);
        }
    }

    fn sort_by(&self, column: Column) {
        if self.with_reader(|reader| reader.sort_by(column)).is_none() {
            return;
        }
        self.items.notify.reset();
        self.show_sort();
        // Keep the current item selected, wherever it moved.
        self.keep_current_item();
    }

    fn show_sort(&self) {
        let sort = self.reader.borrow().sort();
        let window = self.window();
        window.set_sort_column(Column::of(sort.column).index() as i32);
        window.set_sort_ascending(sort.ascending);
    }

    fn item_key(&self, key: &str, page: usize) {
        let key = match key {
            "up" => item_list::Key::Up,
            "down" => item_list::Key::Down,
            "page-up" => item_list::Key::PageUp,
            "page-down" => item_list::Key::PageDown,
            "home" => item_list::Key::Home,
            "end" => item_list::Key::End,
            _ => return,
        };
        let current = usize::try_from(self.window().get_current_item()).ok();
        let len = self.reader.borrow().len();
        if let Some(row) = item_list::step(current, key, len, page) {
            self.item_clicked(row);
        }
    }

    /// Reads the settings file again if it changed. Returns the settings
    /// in effect: the last good ones if the file has errors.
    fn reload_settings(&self) -> Option<Settings> {
        let mut file = self.settings.borrow_mut();
        let file = file.as_mut()?;
        let reload = file.reload();
        if let Some(message) = settings_message(file.path(), &reload) {
            self.status(message);
        } else if reload != Reload::Unchanged {
            self.status("Settings reloaded");
        }
        Some(file.settings().clone())
    }

    fn window_activated(&self) {
        self.reload_settings();
    }

    /// Starts a sync, unless one is running.
    fn start_sync(&self) {
        let Some(database) = self.database.clone() else {
            self.status("Demo data isn't synced");
            return;
        };
        if self.sync.borrow().running.is_some() {
            self.status("A sync is already running");
            return;
        }
        let Some(settings) = self.reload_settings() else {
            return;
        };
        let account = match settings.account() {
            Ok(account) => account,
            Err(err) => {
                let first = err.lines().next().unwrap_or_default();
                self.status_until_next(format!("No account: {first} (settings.toml)"));
                return;
            }
        };
        let password = self
            .sync
            .borrow()
            .password
            .as_ref()
            .filter(|(of, _)| *of == account)
            .map(|(_, password)| password.clone());
        let job = sync_thread::Job {
            database,
            account: account.clone(),
            password,
            options: ren_sync::Options {
                purge_after: settings.keep_read(),
                ..ren_sync::Options::default()
            },
        };
        let weak = self.window.clone();
        let wake = move || {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = weak.upgrade() {
                    window.invoke_sync_event();
                }
            });
        };
        match sync_thread::start(job, wake) {
            Ok(running) => {
                let mut sync = self.sync.borrow_mut();
                sync.running = Some(running);
                sync.account = Some(account);
                sync.refreshed = Some(Instant::now());
                self.window().set_syncing(true);
                self.status_until_next(sync_thread::progress_text(None));
            }
            Err(err) => self.status(format!("Starting the sync failed: {err}")),
        }
    }

    fn cancel_sync(&self) {
        if let Some(running) = &self.sync.borrow().running {
            running.cancel();
            self.status_until_next("Cancelling the sync…");
        }
    }

    /// Handles what the sync thread reported.
    fn sync_events(&self) {
        let events: Vec<Event> = match &self.sync.borrow().running {
            Some(running) => running.events().collect(),
            None => return,
        };
        for event in events {
            match event {
                Event::Password(password) => {
                    let mut sync = self.sync.borrow_mut();
                    sync.password = sync.account.clone().map(|account| (account, password));
                }
                Event::Progress(progress) => {
                    self.status_until_next(sync_thread::progress_text(Some(progress)));
                    // During a full sync, show the items as they arrive.
                    let full = matches!(
                        progress,
                        Progress::Items {
                            expected: Some(_),
                            ..
                        }
                    );
                    let due = self
                        .sync
                        .borrow()
                        .refreshed
                        .is_none_or(|at| at.elapsed() >= SYNC_REFRESH);
                    if full && due {
                        self.sync.borrow_mut().refreshed = Some(Instant::now());
                        self.reload_view();
                    }
                }
                Event::Finished(result) => {
                    {
                        let mut sync = self.sync.borrow_mut();
                        sync.running = None;
                        if result.as_ref().is_err_and(|err| err.unauthorized()) {
                            sync.password = None;
                        }
                    }
                    self.window().set_syncing(false);
                    self.reload_view();
                    let text = sync_thread::result_text(&result);
                    if self.measure {
                        eprintln!("ren: sync result: {text}");
                    }
                    if result.is_ok() {
                        self.status(text);
                    } else {
                        self.status_until_next(text);
                    }
                }
            }
        }
    }

    /// Reads folders, feeds, counts and the list again after the sync
    /// changed them, keeping the selection.
    fn reload_view(&self) {
        let selected = self.reader.borrow().selected();
        let started = Instant::now();
        if self.with_reader(Reader::reload).is_none() {
            return;
        }
        if self.measure {
            eprintln!(
                "ren: lists read again in {:.1} ms ({} items listed)",
                started.elapsed().as_secs_f64() * 1_000.0,
                self.reader.borrow().len()
            );
        }
        self.refresh_tree();
        self.items.notify.reset();
        self.keep_current_item();
        self.update_current_starred();
        if self.reader.borrow().selected() != selected {
            // The feed or folder is gone.
            self.window().set_item_list_content_y(0.0);
        }
    }

    /// Shows the item of a row and marks it read.
    fn item_clicked(&self, row: usize) {
        let window = self.window();
        window.set_current_item(row as i32);
        let Some(opened) = self.with_reader(|reader| reader.open(row)) else {
            return;
        };
        let Some((article, changes)) = opened else {
            self.article.borrow_mut().clear();
            return;
        };
        self.article.borrow_mut().show(article);
        #[cfg(feature = "servo")]
        self.pages.borrow_mut().show_article();
        self.apply(changes);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkKind {
    /// http or https.
    Web,
    Mail,
    /// Anything else (`file:`, `javascript:`, …), which isn't opened.
    Other,
}

fn link_kind(url: &str) -> LinkKind {
    let Some((scheme, rest)) = url.split_once(':') else {
        return LinkKind::Other;
    };
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" if rest.starts_with("//") => LinkKind::Web,
        "mailto" => LinkKind::Mail,
        _ => LinkKind::Other,
    }
}

/// The backend when none is given: Qt draws the Qt style's native widgets.
const DEFAULT_BACKEND: &str = if cfg!(feature = "style-qt") {
    "qt"
} else {
    "winit"
};

/// Uses the given backend, else `DEFAULT_BACKEND`. For winit, uses the
/// given renderer, else the one from `$SLINT_BACKEND`, else
/// `DEFAULT_RENDERER`.
fn select_backend(
    backend: Option<&str>,
    renderer: Option<&str>,
) -> Result<(), slint::PlatformError> {
    let backend = backend.unwrap_or(DEFAULT_BACKEND);
    let mut selector = slint::BackendSelector::new().backend_name(backend.to_owned());
    if backend != "winit" {
        return selector.select();
    }
    let renderer = renderer.or_else(|| {
        std::env::var_os("SLINT_BACKEND")
            .is_none()
            .then_some(DEFAULT_RENDERER)
    });
    if let Some(renderer) = renderer {
        selector = selector.renderer_name(renderer.to_owned());
    }
    selector.select()
}

/// Slint's femtovg renderer on wgpu's Vulkan backend, whose textures Servo's
/// frames can be shared as.
#[cfg(feature = "servo-wgpu")]
fn select_wgpu_backend() -> Result<(), slint::PlatformError> {
    use slint::wgpu_30::{WGPUConfiguration, WGPUSettings, wgpu};
    let mut settings = WGPUSettings::default();
    settings.backends = wgpu::Backends::VULKAN;
    slint::BackendSelector::new()
        .backend_name("winit".to_owned())
        .require_wgpu_30(WGPUConfiguration::Automatic(settings))
        .select()
}

fn log_elapsed(started: Instant, what: &str) {
    eprintln!(
        "ren: {what} after {:.1} ms",
        started.elapsed().as_secs_f64() * 1_000.0
    );
}

/// Reports when the event loop runs and when the first frame has been
/// rendered. Not every renderer supports rendering notifiers (the software
/// renderer, Skia with a software surface); for those only the former is
/// reported, which comes right after the window was shown, but may be before
/// the first frame is on screen.
fn report_startup(window: &MainWindow, started: Instant) {
    let done = Cell::new(false);
    let _ = window.window().set_rendering_notifier(move |state, _| {
        if matches!(state, slint::RenderingState::AfterRendering) && !done.replace(true) {
            log_elapsed(started, "first frame rendered");
        }
    });
    slint::Timer::single_shot(Duration::ZERO, move || {
        log_elapsed(started, "event loop running");
    });
}

/// Scrolls the item list from top to bottom at a constant speed, then quits.
/// Starts after `AUTOSCROLL_DELAY` so that startup work does not end up in
/// the measurement, and updates the position at about 60 Hz.
fn start_autoscroll(window: &MainWindow) -> slint::Timer {
    let timer = slint::Timer::default();
    let weak = window.as_weak();
    let created = Instant::now();
    let mut start = None;
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(16),
        move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            if created.elapsed() < AUTOSCROLL_DELAY {
                return;
            }
            let start = *start.get_or_insert_with(|| {
                eprintln!("ren: autoscroll started");
                Instant::now()
            });
            let range = window.get_item_list_scroll_range();
            let y = start.elapsed().as_secs_f32() * AUTOSCROLL_SPEED;
            window.set_item_list_content_y(-y.min(range));
            if y >= range {
                eprintln!(
                    "ren: autoscroll over {range:.0} px took {:.2} s",
                    start.elapsed().as_secs_f64()
                );
                let _ = slint::quit_event_loop();
            }
        },
    );
    timer
}

/// Time before `--cycle-articles` starts, and between two articles.
const CYCLE_DELAY: Duration = Duration::from_secs(2);
const CYCLE_INTERVAL: Duration = Duration::from_millis(300);
/// Time after the last article, for its images to load.
const CYCLE_SETTLE: Duration = Duration::from_secs(3);

fn log_rss(when: &str) {
    let kib = |field| crate::procstat::proc_status_kib(field).unwrap_or(0);
    eprintln!(
        "ren: rss {when}: {} KiB (anon {}, file {}, shmem {}; peak {} KiB)",
        kib("VmRSS:"),
        kib("RssAnon:"),
        kib("RssFile:"),
        kib("RssShmem:"),
        kib("VmHWM:")
    );
}

/// Opens the first `count` articles of the list one by one, logging memory
/// use before, after the first and after the last, then quits.
fn start_article_cycle(app: std::rc::Weak<App>, count: usize) -> slint::Timer {
    let timer = slint::Timer::default();
    let created = Instant::now();
    let mut opened = 0;
    let mut done_at = None;
    timer.start(slint::TimerMode::Repeated, CYCLE_INTERVAL, move || {
        let Some(app) = app.upgrade() else {
            return;
        };
        if created.elapsed() < CYCLE_DELAY {
            return;
        }
        if let Some(done_at) = done_at {
            if Instant::now() >= done_at {
                if let Some(stats) = app.article.borrow().image_stats() {
                    eprintln!("ren: images: {stats}");
                }
                log_rss(&format!("after {opened} articles"));
                let _ = slint::quit_event_loop();
            }
            return;
        }
        let rows = app.reader.borrow().len();
        if opened == 0 {
            log_rss("before articles");
        }
        if opened < count.min(rows) {
            app.item_clicked(opened);
            opened += 1;
            if opened == 1 {
                log_rss("after 1 article");
            } else if opened.is_multiple_of(100) && opened < count {
                log_rss(&format!("after {opened} articles"));
            }
        } else {
            done_at = Some(Instant::now() + CYCLE_SETTLE);
        }
    });
    timer
}

/// Time before `--measure-sync` starts, and after the last sync.
const MEASURE_SYNC_DELAY: Duration = Duration::from_secs(2);
/// The syncs of `--measure-sync`: into a new database, the first one is
/// a full sync.
const MEASURED_SYNCS: u32 = 2;

/// Runs `MEASURED_SYNCS` syncs one after the other, logging time, CPU and
/// memory use before and after each, then quits.
fn start_sync_measurement(app: std::rc::Weak<App>) -> slint::Timer {
    let timer = slint::Timer::default();
    let created = Instant::now();
    let mut started = 0;
    let mut sync_started = Instant::now();
    let mut cpu_before = None;
    let mut done_at = None;
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        move || {
            let Some(app) = app.upgrade() else {
                return;
            };
            if created.elapsed() < MEASURE_SYNC_DELAY {
                return;
            }
            if let Some(done_at) = done_at {
                if Instant::now() >= done_at {
                    log_rss("idle after the syncs");
                    let _ = slint::quit_event_loop();
                }
                return;
            }
            if app.sync.borrow().running.is_some() {
                return;
            }
            if started > 0 {
                let cpu = crate::procstat::cpu_seconds().zip(cpu_before).map_or(
                    "?".to_owned(),
                    |((user, sys), (user0, sys0))| {
                        format!("{:.2} s user, {:.2} s system", user - user0, sys - sys0)
                    },
                );
                eprintln!(
                    "ren: sync {started} took {:.1} s, CPU {cpu}",
                    sync_started.elapsed().as_secs_f64()
                );
                log_rss(&format!("after sync {started}"));
            } else {
                log_rss("before the syncs");
            }
            if started == MEASURED_SYNCS {
                done_at = Some(Instant::now() + MEASURE_SYNC_DELAY);
                return;
            }
            started += 1;
            sync_started = Instant::now();
            cpu_before = crate::procstat::cpu_seconds();
            app.start_sync();
            if app.sync.borrow().running.is_none() {
                eprintln!(
                    "ren: --measure-sync: the sync didn't start: {}",
                    app.window().get_status_text()
                );
                let _ = slint::quit_event_loop();
            }
        },
    );
    timer
}

/// The message about a settings file that was read, if there is one: its
/// first problem (all of them go to stderr).
fn settings_message(path: &std::path::Path, reload: &Reload) -> Option<String> {
    let problems = match reload {
        Reload::Unchanged => return None,
        Reload::Loaded(warnings) => warnings.as_slice(),
        Reload::Failed(problems) => problems.0.as_slice(),
    };
    for problem in problems {
        eprintln!("ren: {}", crate::account::located(path, problem));
    }
    let first = crate::account::located(path, problems.first()?);
    Some(match reload {
        Reload::Failed(_) => format!("Settings not applied: {first}"),
        _ => first,
    })
}

/// The settings file given, else the default one.
fn settings_path(options: &Options) -> Result<PathBuf, String> {
    match &options.settings {
        Some(path) if !path.exists() => Err(format!("{}: no such file", path.display())),
        Some(path) => Ok(path.clone()),
        None => crate::account::default_path(),
    }
}

/// The database the window shows: the one given, else the default one.
/// Its directory is created if needed.
fn database_path(options: &Options) -> Result<PathBuf, String> {
    let path = match &options.database {
        Some(path) => path.clone(),
        None => ren_settings::paths::database(|name| std::env::var(name).ok())
            .ok_or("cannot find the database: neither XDG_DATA_HOME nor HOME is set".to_owned())?,
    };
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    }
    Ok(path)
}

/// Demo data, if asked for: generated items, or those of a dump.
fn demo_data(options: &Options) -> Result<Option<DemoDb>, String> {
    match (&options.dump, options.items) {
        (Some(dir), items) => {
            DemoDb::from_dump(dir, items.unwrap_or(cli::DEFAULT_DUMP_ITEMS)).map(Some)
        }
        (None, Some(items)) => DemoDb::generated(items).map(Some),
        (None, None) => Ok(None),
    }
}

pub fn run(options: &Options, started: Instant) -> Result<(), String> {
    // Demo data is written before the window is made; the startup times
    // are measured from when it is ready.
    let demo = demo_data(options)?;
    let started = match &demo {
        Some(_) if options.measure => {
            log_elapsed(started, "demo data written");
            Instant::now()
        }
        _ => started,
    };
    let path = match &demo {
        Some(demo) => demo.path().to_owned(),
        None => database_path(options)?,
    };
    let store = Store::open(&path).map_err(|err| format!("{}: {err}", path.display()))?;
    let reader = Reader::new(store).map_err(|err| format!("{}: {err}", path.display()))?;
    // Demo data has neither settings nor a sync.
    let (settings, database) = match demo {
        Some(_) => (None, None),
        None => (
            Some(SettingsFile::open(settings_path(options)?)),
            Some(path),
        ),
    };
    run_window(options, started, reader, settings, database).map_err(|err| err.to_string())
}

fn run_window(
    options: &Options,
    started: Instant,
    reader: Reader,
    settings: Option<(SettingsFile, Reload)>,
    database: Option<PathBuf>,
) -> Result<(), slint::PlatformError> {
    #[cfg(feature = "servo-wgpu")]
    let wgpu_tabs = options.tabs == cli::TabMode::Wgpu;
    #[cfg(not(feature = "servo-wgpu"))]
    let wgpu_tabs = false;
    if wgpu_tabs {
        #[cfg(feature = "servo-wgpu")]
        select_wgpu_backend()?;
    } else {
        select_backend(options.backend.as_deref(), options.renderer.as_deref())?;
    }

    let window = MainWindow::new()?;
    #[cfg(feature = "servo-wgpu")]
    if wgpu_tabs {
        // Before report_startup, which can only set the notifier if this
        // didn't.
        window
            .window()
            .set_rendering_notifier(|state, api| {
                if let (
                    slint::RenderingState::RenderingSetup,
                    slint::GraphicsAPI::WGPU30 { device, .. },
                ) = (state, api)
                {
                    crate::tabs::wgpu::set_device(device.clone());
                }
            })
            .map_err(|err| slint::PlatformError::Other(err.to_string()))?;
    }
    let dark = options.color_scheme.map(|s| s == cli::ColorScheme::Dark);
    if let Some(dark) = dark {
        window.set_forced_color_scheme(if dark {
            ColorScheme::Dark
        } else {
            ColorScheme::Light
        });
    }
    icons::load(&window, !options.bundled_icons, dark);
    window.set_arrangement(match options.arrangement {
        cli::Arrangement::Beside => Arrangement::Beside,
        cli::Arrangement::Above => Arrangement::Above,
    });
    if options.measure {
        log_elapsed(started, "window created");
        report_startup(&window, started);
    }

    let reader = Rc::new(RefCell::new(reader));
    let items = Rc::new(ItemListModel {
        reader: reader.clone(),
        notify: ModelNotify::default(),
    });
    let feeds = Rc::new(VecModel::default());
    window.set_items(ModelRc::from(items.clone()));
    window.set_feeds(ModelRc::from(feeds.clone()));
    let weak = window.as_weak();
    let article = article::ArticlePane::new(
        &window,
        !options.plain_text,
        !options.no_images,
        options.measure || options.cycle_articles.is_some(),
        move |url| {
            if let Some(window) = weak.upgrade() {
                window.invoke_article_link_action(url.into(), "open".into());
            }
        },
    );

    #[cfg(feature = "servo")]
    let pages = pages::Pages::new(
        &window,
        options.tabs,
        options.measure || options.measure_tabs,
    );

    let app = Rc::new(App {
        window: window.as_weak(),
        reader,
        tree_rows: RefCell::new(Vec::new()),
        feeds,
        items,
        article,
        #[cfg(feature = "servo")]
        pages,
        status_timer: slint::Timer::default(),
        database,
        settings: RefCell::new(None),
        sync: RefCell::default(),
        measure: options.measure || options.measure_sync,
    });
    app.refresh_tree();
    app.show_sort();
    if let Some((file, reload)) = settings {
        if let Some(message) = settings_message(file.path(), &reload) {
            app.status_until_next(message);
        }
        *app.settings.borrow_mut() = Some(file);
    }
    if app.database.is_some() && app.reader.borrow().tree_rows().len() <= 2 {
        app.status_until_next(
            "No feeds yet: Feed → Fetch All Feeds (Ctrl+L) syncs with the server",
        );
    }

    let weak = Rc::downgrade(&app);
    window.on_feed_clicked(move |row| {
        if let Some(app) = weak.upgrade() {
            app.feed_clicked(row as usize);
        }
    });
    let weak = Rc::downgrade(&app);
    window.on_feed_toggled(move |row| {
        if let Some(app) = weak.upgrade() {
            app.feed_toggled(row as usize);
        }
    });
    let weak = Rc::downgrade(&app);
    window.on_feed_key(move |key| {
        if let Some(app) = weak.upgrade() {
            app.feed_key(&key);
        }
    });
    let weak = Rc::downgrade(&app);
    window.on_item_clicked(move |row| {
        if let Some(app) = weak.upgrade() {
            app.item_clicked(row as usize);
        }
    });

    let weak = Rc::downgrade(&app);
    window.on_item_key(move |key, page| {
        if let Some(app) = weak.upgrade() {
            app.item_key(&key, usize::try_from(page).unwrap_or(1));
        }
    });
    let weak = Rc::downgrade(&app);
    window.on_sort_clicked(move |column| {
        if let Some(app) = weak.upgrade() {
            app.sort_clicked(column as usize);
        }
    });

    let weak = Rc::downgrade(&app);
    window.on_action(move |name| {
        if let Some(app) = weak.upgrade() {
            app.action(&name);
        }
    });
    let weak = Rc::downgrade(&app);
    window.on_feed_menu(move |row, action| {
        if let Some(app) = weak.upgrade() {
            app.feed_menu(row as usize, &action);
        }
    });
    let weak = Rc::downgrade(&app);
    window.on_item_menu(move |row, action| {
        if let Some(app) = weak.upgrade() {
            app.item_menu(row as usize, &action);
        }
    });
    let weak = Rc::downgrade(&app);
    window.on_article_link_action(move |url, action| {
        if let Some(app) = weak.upgrade() {
            app.article_link(&url, &action);
        }
    });
    let weak = Rc::downgrade(&app);
    window.on_sync_event(move || {
        if let Some(app) = weak.upgrade() {
            app.sync_events();
        }
    });
    let weak = Rc::downgrade(&app);
    window.on_window_activated(move || {
        if let Some(app) = weak.upgrade() {
            app.window_activated();
        }
    });
    window.set_tabs_available(cfg!(feature = "servo"));
    let weak = Rc::downgrade(&app);
    window.on_search_edited(move |text| {
        if let Some(app) = weak.upgrade() {
            app.search_edited(&text);
        }
    });

    let _autoscroll = options.autoscroll.then(|| start_autoscroll(&window));
    let _measure_sync = options
        .measure_sync
        .then(|| start_sync_measurement(Rc::downgrade(&app)));
    let _cycle = options
        .cycle_articles
        .map(|count| start_article_cycle(Rc::downgrade(&app), count));
    #[cfg(feature = "servo")]
    let _measure_tabs = options.measure_tabs.then(|| {
        let urls = if options.tab_urls.is_empty() {
            let reader = app.reader.borrow();
            (0..reader.len())
                .filter_map(|row| reader.url(reader.id(row)?).ok().flatten())
                .take(3)
                .collect()
        } else {
            options.tab_urls.clone()
        };
        if urls.is_empty() {
            eprintln!("ren: --measure-tabs needs --tab-url or items with links (--dump)");
            std::process::exit(1);
        }
        pages::start_measurement(Rc::downgrade(&app.pages), urls)
    });
    window.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_kinds() {
        assert_eq!(link_kind("https://example.org/a"), LinkKind::Web);
        assert_eq!(link_kind("HTTP://example.org"), LinkKind::Web);
        assert_eq!(link_kind("mailto:a@example.org"), LinkKind::Mail);
        assert_eq!(link_kind("file:///etc/passwd"), LinkKind::Other);
        assert_eq!(link_kind("javascript:alert(1)"), LinkKind::Other);
        assert_eq!(link_kind("http:no-slashes"), LinkKind::Other);
        assert_eq!(link_kind("http"), LinkKind::Other);
        assert_eq!(link_kind(""), LinkKind::Other);
    }
}
