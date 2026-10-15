// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Slint glue: binds the view models to the main window.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use ren_settings::{Reload, Settings, SettingsFile};
use ren_store::{Status, Store};
use ren_sync::{FeedChange, Progress};
use slint::language::ColorScheme;
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, VecModel};

use crate::cli::{self, Options};
use crate::demo::DemoDb;
use crate::favicons::Favicons;
use crate::feed_tree::{self, Node, Row, changed_rows};
use crate::instance::{Activate, Claim, Instance};
use crate::item_list::{self, Column, format_date};
use crate::reader::{Changes, Reader};
use crate::sync_thread::{self, Event, Running};
use crate::window_state::{self, Saved};

mod about;
mod account;
mod article;
mod feeds;
#[cfg(test)]
mod flows;
mod geometry;
mod icons;
#[cfg(feature = "servo")]
mod pages;
mod settings;

slint::include_modules!();

/// The application's id: of its `.desktop` file, icon and AppStream
/// metadata (`data/`), and of its windows, by which Wayland compositors
/// find the icon.
const APP_ID: &str = "io.github.pprkut.ren";

/// The renderer with the smallest footprint, see
/// `docs/decisions/0001-ui-toolkit.md`.
const DEFAULT_RENDERER: &str = "software";

/// How long a message stays in the status bar.
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

/// Scroll speed of `--autoscroll`, in logical pixels per second.
const AUTOSCROLL_SPEED: f32 = 8_000.0;
/// Time between startup and the start of `--autoscroll`.
const AUTOSCROLL_DELAY: Duration = Duration::from_secs(3);

/// The icon in the system tray: the logo for small sizes, in colour, which
/// shows on light and dark panels, or in one colour.
const TRAY_LOGO: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/icons/ren-small.svg"));
const TRAY_LOGO_MONO: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/icons/ren-mono.svg"));

/// The size the tray icon is drawn at, in pixels. Slint sends the tray one
/// pixmap, which the panel scales: Plasma shows tray icons at 22 pixels,
/// so 44 is their size at a scale of 2 and halves cleanly to 22, where 64
/// came out blurred at both.
const TRAY_ICON_SIZE: u32 = 44;

/// The colours of the monochrome tray icon: Breeze's text colours on a dark
/// and a light panel.
const TRAY_LIGHT: &str = "#fcfcfc";
const TRAY_DARK: &str = "#232629";

/// Time after the window was moved or resized until its size and position
/// are kept: the window manager has updated its maximised state by then.
const GEOMETRY_DELAY: Duration = Duration::from_millis(300);

/// Time after the window was hidden until freed memory is given back.
const HIDDEN_TRIM_DELAY: Duration = Duration::from_secs(1);

/// Time between the start and the first automatic sync, so that it
/// doesn't compete with the first frames.
const FIRST_SYNC_DELAY: Duration = Duration::from_secs(3);

/// Time between the start and opening the account page on the first
/// run: after the main window is mapped, so that the dialog comes in
/// front of it (Slint can't make it a transient window of the main one).
const FIRST_RUN_DELAY: Duration = Duration::from_millis(500);

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

/// A row of the feed tree; `full_page` for feeds whose articles show their
/// web page instead, `icon` a feed's own icon (or none).
fn feed_row(row: &Row, full_page: bool, icon: slint::Image) -> FeedRow {
    FeedRow {
        title: row.title.as_str().into(),
        depth: row.depth.into(),
        count: row.count.try_into().unwrap_or(i32::MAX),
        starred: row.node == Node::Starred,
        is_folder: row.expanded.is_some(),
        expanded: row.expanded == Some(true),
        last: row.last,
        guides: ModelRc::new(VecModel::from(row.guides.clone())),
        full_page,
        icon,
        broken_list: row.node == Node::Broken,
        error: row.error.as_deref().unwrap_or_default().into(),
    }
}

struct App {
    /// The app itself, for callbacks of other windows.
    this: std::rc::Weak<App>,
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
    /// `--no-images`: articles never load images from their servers.
    no_images: bool,
    /// The window's state, written when the window closes.
    saved: RefCell<Saved>,
    /// The arrangement whose pane sizes the window shows.
    arrangement: Cell<Arrangement>,
    /// `--arrangement` was given: the setting is neither followed nor
    /// changed.
    arrangement_forced: bool,
    /// Starts the automatic syncs (`sync.interval-minutes`), counted from
    /// the end of the last sync.
    sync_timer: slint::Timer,
    /// The interval `sync_timer` runs at; `None` while it is stopped.
    sync_interval: Cell<Option<Duration>>,
    /// Syncs start by themselves; not with demo data or while measuring.
    auto_sync: bool,
    /// The feeds' icons; `None` without a cache directory.
    favicons: Option<Favicons>,
    /// The icon in the system tray, if `window.tray-icon` is on.
    tray: RefCell<Option<Tray>>,
    /// How the tray icon is drawn (`window.tray-icon-style`).
    tray_style: Cell<ren_settings::TrayIconStyle>,
    /// Closing the window hides it in the tray (`window.close-to-tray`).
    close_to_tray: Cell<bool>,
    /// The window is hidden in the tray.
    hidden: Cell<bool>,
    /// A tray icon may be shown: not while measuring.
    tray_allowed: bool,
    /// Keeps the window's size and position a moment after it moved.
    geometry_timer: slint::Timer,
    /// The settings dialog, while it is open.
    dialog: RefCell<Option<settings::SettingsDialog>>,
    /// Why the settings file can't be used or changed: its errors.
    settings_problem: RefCell<Option<String>>,
    /// Watches the settings file, so that it is read when it was saved.
    settings_watch: RefCell<Option<ren_settings::Watch>>,
    /// Reads the settings file a moment after it was saved.
    reload_timer: slint::Timer,
    /// `--color-scheme`: dark (or light) instead of the desktop's.
    forced_dark: Option<bool>,
    /// A login or logout of the account page, while it runs.
    login: RefCell<Option<crate::login::Running>>,
    account_ui: RefCell<account::AccountUi>,
    /// Why the last sync had no app password, or had it refused: the
    /// account page asks to log in again.
    password_problem: RefCell<Option<String>>,
    /// A sync finished since the start.
    synced_once: Cell<bool>,
    /// Why the last sync failed, if it did, for the welcome page.
    sync_failure: RefCell<Option<String>>,
    /// Help → About Ren, while it is open.
    about: RefCell<Option<AboutWindow>>,
    /// The question before logging out or deleting, while it is open.
    confirm: RefCell<Option<ConfirmWindow>>,
    /// Changes to the feeds and folders for the next sync.
    feed_changes: RefCell<Vec<FeedChange>>,
    /// Adding or editing a feed or a folder, while it is open.
    feed_dialog: RefCell<Option<feeds::Dialog>>,
    /// The feed added, shown after the sync that brings it in.
    select_after_sync: Cell<Option<Node>>,
    /// A feed at a new address that shows full pages, as the old one did.
    full_page_after_sync: Cell<Option<u64>>,
    /// What the server refused of the changes sent with the running sync.
    refused_changes: RefCell<Vec<String>>,
}

#[derive(Default)]
struct SyncState {
    running: Option<Running>,
    /// The account the running sync is for.
    account: Option<ren_settings::Account>,
    /// The app password from the last sync or login, with the account it
    /// is for and where it came from.
    password: Option<(ren_settings::Account, String, ren_settings::Source)>,
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

    /// Shows what was kept from the last run: the list, its order and the
    /// collapsed folders, the item shown (without marking it read), the
    /// pane sizes. Before the window is shown.
    fn restore(&self) {
        let (view, item) = {
            let saved = self.saved.borrow();
            let state = saved.state();
            let item = state.selection.and_then(|selection| selection.item);
            (window_state::view(state), item)
        };
        self.with_reader(|reader| reader.restore(&view));
        self.refresh_tree();
        self.show_sort();
        self.apply_layout(self.arrangement.get());
        let row = item.and_then(|id| self.reader.borrow().row_of(id));
        if let Some(row) = row {
            self.window().set_current_item(row as i32);
            // The web page of a full-page feed isn't loaded: that would
            // start Servo at start.
            if let Some(Some(article)) = self.with_reader(|reader| reader.show(row)) {
                self.article.borrow_mut().show(article);
                self.update_current_starred();
            }
        }
    }

    /// Keeps what the window shows in the state, to be written when it
    /// closes.
    fn remember(&self) {
        let window = self.window();
        let (view, item) = {
            let reader = self.reader.borrow();
            (reader.view(), reader.current())
        };
        let mut saved = self.saved.borrow_mut();
        saved.change(|state| {
            window_state::set_view(state, &view, item);
            state.window = geometry::current(window.window(), &state.window);
        });
        drop(saved);
        self.stash_layout(self.arrangement.get());
    }

    /// Keeps the window's size and position once it settled after a move
    /// or a resize, so the size it had before it was maximised is known
    /// when it closes maximised.
    fn window_moved(&self) {
        let weak = self.window.clone();
        self.geometry_timer
            .start(slint::TimerMode::SingleShot, GEOMETRY_DELAY, move || {
                if let Some(window) = weak.upgrade() {
                    window.invoke_action("window-moved".into());
                }
            });
    }

    fn remember_geometry(&self) {
        if self.hidden.get() {
            return;
        }
        let window = self.window();
        self.saved.borrow_mut().change(|state| {
            state.window = geometry::current(window.window(), &state.window);
        });
    }

    /// Keeps the pane sizes and column widths of an arrangement.
    fn stash_layout(&self, arrangement: Arrangement) {
        let window = self.window();
        let list_size = match arrangement {
            Arrangement::Beside => window.get_list_width(),
            Arrangement::Above => window.get_list_height(),
        };
        self.saved.borrow_mut().change(|state| {
            let layout = layout_mut(state, arrangement);
            layout.tree_width = Some(window.get_tree_width());
            layout.list_size = Some(list_size);
            layout.columns = ren_settings::Columns {
                feed: Some(window.get_feed_width()),
                author: Some(window.get_author_width()),
                date: Some(window.get_date_width()),
            };
        });
    }

    /// Shows the pane sizes and column widths kept for an arrangement.
    fn apply_layout(&self, arrangement: Arrangement) {
        let window = self.window();
        let layout = layout(self.saved.borrow().state(), arrangement).clone();
        if let Some(width) = layout.tree_width {
            window.set_tree_width(width);
        }
        if let Some(size) = layout.list_size {
            match arrangement {
                Arrangement::Beside => window.set_list_width(size),
                Arrangement::Above => window.set_list_height(size),
            }
        }
        let columns = &layout.columns;
        if let Some(width) = columns.feed {
            window.set_feed_width(width);
        }
        if let Some(width) = columns.author {
            window.set_author_width(width);
        }
        if let Some(width) = columns.date {
            window.set_date_width(width);
        }
    }

    /// The View menu switched the arrangement: the sizes of the old one are
    /// kept, those of the new one shown.
    fn arrangement_changed(&self) {
        let new = self.window().get_arrangement();
        let old = self.arrangement.replace(new);
        if old != new {
            self.stash_layout(old);
            self.apply_layout(new);
        }
        // The View menu changes the setting.
        let above = new == Arrangement::Above;
        let differs = self
            .settings
            .borrow()
            .as_ref()
            .is_some_and(|file| file.settings().list_above() != above);
        if differs && !self.arrangement_forced {
            let value = if above { "above" } else { "beside" };
            self.change_setting(
                ren_settings::schema::ARRANGEMENT,
                Some(&ren_settings::Value::Text(value.to_owned())),
            );
        }
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
                    self.feeds.set_row_data(i, self.feed_row(&rows[i]));
                }
            }
            None => self.feeds.set_vec(
                rows.iter()
                    .map(|row| self.feed_row(row))
                    .collect::<Vec<_>>(),
            ),
        }
        let current = rows.iter().position(|r| r.node == selected);
        *self.tree_rows.borrow_mut() = rows;
        let window = self.window();
        window.set_current_feed(current.map_or(-1, |i| i as i32));
        window.set_unread_text(format!("{unread} unread articles").into());
        if let Some(tray) = &*self.tray.borrow() {
            tray.set_text(format!("Ren: {unread} unread articles").into());
        }
        self.refresh_welcome();
    }

    /// Updates what the welcome page says: the counts, the last sync,
    /// whether there is an account.
    fn refresh_welcome(&self) {
        let Some(overview) = self.with_reader(|reader| reader.overview()) else {
            return;
        };
        let window = self.window();
        window.set_welcome_articles(crate::welcome::articles_text(&overview).into());
        let failure = self.sync_failure.borrow();
        window.set_welcome_sync(crate::welcome::sync_text(&overview, failure.as_deref()).into());
        window.set_logged_in(self.current_settings().account().is_ok());
    }

    /// Shows or removes the icon in the system tray.
    fn set_tray(&self, shown: bool) {
        let mut tray = self.tray.borrow_mut();
        if shown == tray.is_some() {
            return;
        }
        if !shown {
            *tray = None;
            drop(tray);
            // Without the tray, a hidden window couldn't come back.
            if self.hidden.get() {
                self.show_window();
            }
            return;
        }
        let created = match Tray::new() {
            Ok(created) => created,
            Err(err) => {
                eprintln!("ren: no icon in the system tray: {err}");
                return;
            }
        };
        created.set_window_shown(!self.hidden.get());
        created.set_syncing(self.sync.borrow().running.is_some());
        let weak = self.window.clone();
        created.on_toggle_window(move || {
            if let Some(window) = weak.upgrade() {
                window.invoke_action("toggle-window".into());
            }
        });
        let weak = self.window.clone();
        created.on_action(move |action| {
            if let Some(window) = weak.upgrade() {
                window.invoke_action(action);
            }
        });
        *tray = Some(created);
        drop(tray);
        self.update_tray_icon();
        self.refresh_tree();
    }

    /// Draws the tray icon in its style; a monochrome one following the
    /// colour scheme also after the scheme changed.
    fn update_tray_icon(&self) {
        if let Some(tray) = &*self.tray.borrow() {
            let svg = tray_svg(self.tray_style.get(), self.window().get_dark());
            match slint::Image::load_from_svg_data(svg.as_bytes()) {
                Ok(icon) => tray.set_image(icon),
                Err(err) => eprintln!("ren: the tray icon: {err:?}"),
            }
        }
    }

    /// The window was asked to close: it hides in the tray, or ren quits.
    /// Only a desktop that shows tray icons gets the window back from the
    /// tray: Slint makes the icon either way.
    fn close_requested(&self) -> slint::CloseRequestResponse {
        self.remember();
        let to_tray = self.close_to_tray.get() && self.tray.borrow().is_some();
        if to_tray && tray_host() {
            self.hide_window();
        } else {
            if to_tray {
                eprintln!("ren: the desktop shows no tray icons; quitting instead");
            }
            let _ = slint::quit_event_loop();
        }
        slint::CloseRequestResponse::HideWindow
    }

    /// Hides the window in the tray. What only a shown window needs goes:
    /// the article's view and frames, the tabs (Servo's helper); the sync
    /// goes on.
    fn hide_window(&self) {
        if self.hidden.replace(true) {
            return;
        }
        self.remember();
        let _ = self.window().hide();
        self.article.borrow_mut().suspend();
        #[cfg(feature = "servo")]
        self.pages.borrow_mut().suspend();
        if let Some(tray) = &*self.tray.borrow() {
            tray.set_window_shown(false);
        }
        slint::Timer::single_shot(HIDDEN_TRIM_DELAY, || {
            crate::procstat::trim_heap();
        });
    }

    /// Shows the window again where it was, with its article and tabs.
    fn show_window(&self) {
        if !self.hidden.replace(false) {
            return;
        }
        let window = self.window();
        let saved = self.saved.borrow().state().window.clone();
        geometry::restore(window.window(), &saved);
        if let Err(err) = window.show() {
            eprintln!("ren: showing the window: {err}");
        }
        geometry::place(self.window.clone(), saved);
        self.article.borrow_mut().resume();
        #[cfg(feature = "servo")]
        self.pages.borrow_mut().resume();
        if let Some(tray) = &*self.tray.borrow() {
            tray.set_window_shown(true);
        }
    }

    /// Shows in the window and the tray whether a sync runs.
    fn set_syncing(&self, syncing: bool) {
        self.window().set_syncing(syncing);
        if let Some(tray) = &*self.tray.borrow() {
            tray.set_syncing(syncing);
        }
    }

    /// Ren was started again: the window comes to the front, also from the
    /// tray. X11 raises and focuses it; Wayland lets only the launcher's
    /// activation token (`token`) do that, which winit can't pass on, so
    /// there the window asks for attention (the task bar highlights it),
    /// unless it was hidden: a window shown again gets the focus.
    fn activate(&self, token: &str) {
        if self.hidden.get() {
            self.show_window();
            return;
        }
        let _ = token;
        let window = self.window();
        if window.window().is_minimized() {
            window.window().set_minimized(false);
        }
        use slint::winit_030::WinitWindowAccessor;
        window.window().with_winit_window(|winit_window| {
            winit_window.focus_window();
            winit_window.request_user_attention(Some(
                slint::winit_030::winit::window::UserAttentionType::Informational,
            ));
        });
    }

    fn toggle_window(&self) {
        if self.hidden.get() {
            self.show_window();
        } else {
            self.hide_window();
        }
    }

    fn feed_row(&self, row: &Row) -> FeedRow {
        let Node::Feed(id) = row.node else {
            return feed_row(row, false, slint::Image::default());
        };
        let reader = self.reader.borrow();
        let icon = self
            .favicons
            .as_ref()
            .zip(reader.favicon_link(id))
            .and_then(|(favicons, link)| favicons.icon(link))
            .and_then(|path| slint::Image::load_from_path(&path).ok())
            .unwrap_or_default();
        feed_row(row, reader.is_full_page(id), icon)
    }

    /// Shows the feeds' icons again, after some were downloaded.
    fn reload_icons(&self) {
        let rows: Vec<FeedRow> = self
            .tree_rows
            .borrow()
            .iter()
            .map(|row| self.feed_row(row))
            .collect();
        self.feeds.set_vec(rows);
    }

    /// Downloads the icons of the feeds that have none yet or an old one,
    /// unless `sync.feed-icons` is off.
    fn update_favicons(&self) {
        let Some(favicons) = &self.favicons else {
            return;
        };
        let enabled = self
            .settings
            .borrow()
            .as_ref()
            .is_some_and(|file| file.settings().feed_icons());
        if !enabled {
            return;
        }
        let due = favicons.due(self.reader.borrow().favicon_links(), SystemTime::now());
        let weak = self.window.clone();
        favicons.update(due, move |changed| {
            if changed {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(window) = weak.upgrade() {
                        window.invoke_action("favicons-updated".into());
                    }
                });
            }
        });
    }

    /// Switches whether a feed's articles show their web page instead.
    fn toggle_full_page(&self, row: usize, feed_id: u64) {
        let full_page = !self.reader.borrow().is_full_page(feed_id);
        if self
            .with_reader(|reader| reader.set_full_page(feed_id, full_page))
            .is_some()
            && let Some(tree_row) = self.tree_rows.borrow().get(row)
        {
            self.feeds.set_row_data(row, self.feed_row(tree_row));
        }
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

    /// Marks a read item unread, an unread one read.
    fn toggle_read(&self, id: u64) {
        if let Some(changes) = self.with_reader(|reader| reader.toggle_read(id)) {
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
                if !self.hidden.get() {
                    self.remember();
                }
                let _ = slint::quit_event_loop();
            }
            "toggle-window" => self.toggle_window(),
            "theme-changed" => self.update_tray_icon(),
            "window-moved" => self.remember_geometry(),
            "sort-title" => self.sort_by(Column::Title),
            "sort-feed" => self.sort_by(Column::Feed),
            "sort-author" => self.sort_by(Column::Author),
            "sort-date" => self.sort_by(Column::Date),
            "previous-article" => self.item_key("up", 1),
            "next-article" => self.item_key("down", 1),
            "next-unread" => self.next_unread(),
            "load-images" => self.article.borrow_mut().load_images_once(),
            "mark-feed-read" => {
                // Not borrowed while marking: the reader is borrowed
                // mutably then.
                let selected = self.reader.borrow().selected();
                self.mark_node_read(selected);
            }
            "mark-all-read" => self.mark_node_read(Node::All),
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
            "toggle-read" => {
                if let Some(id) = current {
                    self.toggle_read(id);
                }
            }
            "open-tab" | "open-browser" => {
                if let Some(id) = current {
                    self.open_item(id, name == "open-browser");
                }
            }
            "fetch-feed" | "fetch-all" => self.start_sync(),
            "auto-sync" => self.auto_sync(),
            "favicons-updated" => self.reload_icons(),
            "cancel-sync" => self.cancel_sync(),
            #[cfg(feature = "servo")]
            "close-tab" => self.pages.borrow_mut().close_current(),
            #[cfg(feature = "servo")]
            "page-browser" => {
                let url = self.pages.borrow().current_url();
                if let Some(url) = url {
                    self.open_in_browser(&url);
                }
            }
            #[cfg(feature = "servo")]
            "toggle-site-blocking" => self.toggle_site_blocking(),
            #[cfg(feature = "servo")]
            "clear-site-data" => {
                let result = self.pages.borrow_mut().clear_site_data();
                match result {
                    Ok(()) => self.status("Cookies and site data of web pages removed"),
                    Err(err) => self.status(format!("Removing the site data failed: {err}")),
                }
            }
            "configure" => self.open_settings(),
            "settings-saved" => self.settings_saved(),
            "login-event" => self.login_events(),
            "add-feed" => self.add_feed(),
            "about" => self.open_about(),
            "about-close" => self.close_about(),
            _ => self.not_implemented(name),
        }
    }

    fn feed_menu(&self, row: usize, action: &str) {
        match (action, self.node(row)) {
            ("mark-feed-read", Some(node)) => self.mark_node_read(node),
            ("toggle-full-page", Some(Node::Feed(id))) => self.toggle_full_page(row, id),
            // The server fetches the feeds; ren syncs everything.
            ("fetch-feed" | "fetch-all", _) => self.start_sync(),
            (_, Some(node)) if self.manage_feeds(node, action) => {}
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
            "copy-link" => match self.with_reader(|reader| reader.url(id)) {
                Some(Some(url)) => self.window().invoke_copy_text(url.into()),
                Some(None) => self.status("This article has no link"),
                None => {}
            },
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
        self.xdg_open(url.as_ref());
    }

    /// Opens a URL or file with the desktop's default application.
    fn xdg_open(&self, target: &std::ffi::OsStr) {
        match std::process::Command::new("xdg-open")
            .arg(target)
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

    /// Blocks ads and trackers on the site of the page shown, or stops
    /// blocking them there (`pages.unblocked-sites`), and reloads it.
    #[cfg(feature = "servo")]
    fn toggle_site_blocking(&self) {
        use crate::tabs::filters;
        let Some(host) = self.pages.borrow().current_host() else {
            return;
        };
        let changed = {
            let mut file = self.settings.borrow_mut();
            let Some(file) = file.as_mut() else {
                self.status("Demo data has no settings");
                return;
            };
            let (sites, unblocked, site) =
                filters::toggle_site(file.settings().unblocked_sites(), &host);
            file.set(
                ren_settings::schema::UNBLOCKED_SITES,
                &ren_settings::Value::List(sites),
            )
            .map(|()| (file.settings().clone(), unblocked, site))
        };
        match changed {
            Ok((settings, unblocked, site)) => {
                self.apply_settings(&settings);
                self.pages.borrow_mut().navigate("reload");
                self.status(if unblocked {
                    format!("Ads and trackers are blocked on {site} again")
                } else {
                    format!("Ads and trackers aren't blocked on {site}")
                });
            }
            Err(err) => self.status(format!("Changing the settings failed: {err}")),
        }
    }

    /// Reads the settings file again if it changed. Returns the settings
    /// in effect: the last good ones if the file has errors.
    fn reload_settings(&self) -> Option<Settings> {
        let (reload, settings) = {
            let mut file = self.settings.borrow_mut();
            let file = file.as_mut()?;
            let reload = file.reload();
            if let Some(message) = settings_message(file.path(), &reload) {
                self.status(message);
            } else if reload != Reload::Unchanged {
                self.status("Settings reloaded");
            }
            self.note_settings_problem(file.path(), &reload);
            (reload, file.settings().clone())
        };
        if reload != Reload::Unchanged {
            self.apply_settings(&settings);
            self.refresh_settings_dialog();
        }
        Some(settings)
    }

    /// Keeps the errors of a settings file that was read, for the
    /// settings dialog; a usable file has none.
    fn note_settings_problem(&self, path: &std::path::Path, reload: &Reload) {
        let mut problem = self.settings_problem.borrow_mut();
        match reload {
            Reload::Unchanged => {}
            Reload::Loaded(_) => *problem = None,
            Reload::Failed(problems) => {
                let located: Vec<String> = problems
                    .0
                    .iter()
                    .map(|problem| crate::account::located(path, problem))
                    .collect();
                *problem = Some(settings::file_problem(&located.join("\n")));
            }
        }
    }

    /// Applies the settings the window uses itself; the sync reads its own
    /// when it starts.
    fn apply_settings(&self, settings: &Settings) {
        let window = self.window();
        if !self.arrangement_forced {
            window.set_arrangement(arrangement_of(settings));
        }
        let theme = window.global::<Theme>();
        let colour = |key| {
            settings
                .colour(key)
                .map(|c| slint::Color::from_rgb_u8(c.red, c.green, c.blue))
        };
        let new = colour(ren_settings::schema::NEW_COLOUR);
        theme.set_custom_new(new.is_some());
        theme.set_new_colour(new.unwrap_or_default());
        let unread = colour(ren_settings::schema::UNREAD_COLOUR);
        theme.set_custom_unread(unread.is_some());
        theme.set_unread_colour(unread.unwrap_or_default());
        self.article
            .borrow_mut()
            .set_image_cache_bytes(settings.image_cache_bytes());
        self.schedule_syncs(settings.sync_interval());
        self.close_to_tray.set(settings.close_to_tray());
        let style = settings.tray_icon_style();
        let restyled = self.tray_style.replace(style) != style;
        self.set_tray(settings.tray_icon() && self.tray_allowed);
        if restyled {
            self.update_tray_icon();
        }
        self.article
            .borrow_mut()
            .set_load_images(settings.load_images() && !self.no_images);
        #[cfg(feature = "servo")]
        {
            let mut pages = self.pages.borrow_mut();
            pages.set_keep_site_data(settings.keep_site_data());
            pages.set_blocking(settings.filter_lists(), settings.unblocked_sites());
        }
        // Logging in or out shows or hides the welcome page's button.
        self.refresh_welcome();
    }

    /// Starts the automatic syncs at `interval`, or stops them. A new
    /// interval counts from now.
    fn schedule_syncs(&self, interval: Option<Duration>) {
        let interval = interval.filter(|_| self.auto_sync);
        if self.sync_interval.replace(interval) == interval {
            return;
        }
        match interval {
            Some(interval) => {
                let weak = self.window.clone();
                self.sync_timer
                    .start(slint::TimerMode::Repeated, interval, move || {
                        if let Some(window) = weak.upgrade() {
                            window.invoke_action("auto-sync".into());
                        }
                    });
            }
            None => self.sync_timer.stop(),
        }
    }

    /// Starts a sync, unless one is running.
    fn start_sync(&self) {
        self.sync_now(true);
    }

    /// Starts an automatic sync: as a sync asked for, but quietly skipped
    /// while one runs or no account is set up.
    fn auto_sync(&self) {
        if self.auto_sync {
            self.sync_now(false);
        }
    }

    fn sync_now(&self, asked: bool) {
        let Some(database) = self.database.clone() else {
            self.status("Demo data isn't synced");
            return;
        };
        if self.sync.borrow().running.is_some() {
            if asked {
                self.status("A sync is already running");
            }
            return;
        }
        let Some(settings) = self.reload_settings() else {
            return;
        };
        let account = match settings.account() {
            Ok(account) => account,
            Err(_) if !asked => return,
            Err(_) => {
                self.status("Log in to your Nextcloud first");
                self.open_account_page();
                return;
            }
        };
        let password = self
            .sync
            .borrow()
            .password
            .as_ref()
            .filter(|(of, _, _)| *of == account)
            .map(|(_, password, _)| password.clone());
        let job = sync_thread::Job {
            database,
            account: account.clone(),
            password,
            options: ren_sync::Options {
                purge_after: settings.keep_read(),
                ..ren_sync::Options::default()
            },
            changes: std::mem::take(&mut self.feed_changes.borrow_mut()),
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
                self.set_syncing(true);
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
                Event::Password(password, source) => {
                    let mut sync = self.sync.borrow_mut();
                    sync.password = sync
                        .account
                        .clone()
                        .map(|account| (account, password, source));
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
                Event::Changed(change, result) => self.feed_changed(change, result),
                Event::Finished(result) => {
                    match &result {
                        Ok(_) => *self.sync_failure.borrow_mut() = None,
                        Err(sync_thread::Failure::Sync(ren_sync::Error::Cancelled)) => {}
                        Err(err) => *self.sync_failure.borrow_mut() = Some(err.to_string()),
                    }
                    {
                        let mut sync = self.sync.borrow_mut();
                        sync.running = None;
                        if result.as_ref().is_err_and(|err| err.unauthorized()) {
                            sync.password = None;
                        }
                    }
                    self.check_password(&result);
                    self.set_syncing(false);
                    // The next automatic sync is an interval after this one.
                    if self.sync_timer.running() {
                        self.sync_timer.restart();
                    }
                    self.reload_view();
                    if result.is_ok() {
                        self.update_favicons();
                    }
                    let mut text = sync_thread::result_text(&result);
                    if self.measure {
                        eprintln!("ren: sync result: {text}");
                    }
                    // What the server refused of the changes to the feeds
                    // stays in the status bar.
                    let refused = std::mem::take(&mut *self.refused_changes.borrow_mut());
                    for refusal in &refused {
                        text.push_str("; ");
                        text.push_str(refusal);
                    }
                    if result.is_ok() && refused.is_empty() {
                        self.status(text);
                    } else {
                        self.status_until_next(text);
                    }
                    let failure = result.as_ref().err().map(ToString::to_string);
                    self.feeds_after_sync(failure);
                }
            }
        }
    }

    /// After a sync: whether it had an app password that the server took.
    /// If not, the account page asks to log in, and opens by itself after
    /// the first sync since the start (the password may be in a keyring
    /// that isn't there, as without a Secret Service).
    fn check_password(&self, result: &Result<ren_sync::Report, sync_thread::Failure>) {
        let problem = match result {
            Err(sync_thread::Failure::Password(err)) => {
                Some(format!("The sync found no app password: {err}"))
            }
            Err(err) if err.unauthorized() => Some(
                "The server refused the app password; it may have been revoked. Log in again."
                    .to_owned(),
            ),
            Err(_) => return,
            Ok(_) => None,
        };
        let first = !self.synced_once.replace(true);
        let open = problem.is_some() && first;
        *self.password_problem.borrow_mut() = problem;
        if open {
            self.open_account_page();
        } else {
            self.refresh_settings_dialog();
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
        #[cfg(feature = "servo")]
        self.show_page_or_article(article.url.as_deref());
        self.article.borrow_mut().show(article);
        self.apply(changes);
    }

    /// Shows the article in front of the tabs, or for feeds set to show
    /// full pages, the article's web page in a tab (the article is still
    /// in the Article tab).
    #[cfg(feature = "servo")]
    fn show_page_or_article(&self, url: Option<&str>) {
        let full_page = self.reader.borrow().current_full_page();
        let url = url.filter(|url| full_page && link_kind(url) == LinkKind::Web);
        let mut pages = self.pages.borrow_mut();
        match url.map(|url| pages.open_for_article(url)) {
            Some(Ok(())) => {}
            Some(Err(err)) => {
                pages.show_article();
                drop(pages);
                self.status(format!("Opening the page failed: {err}"));
            }
            None => pages.show_article(),
        }
    }
}

/// The arrangement the settings ask for.
fn arrangement_of(settings: &Settings) -> Arrangement {
    if settings.list_above() {
        Arrangement::Above
    } else {
        Arrangement::Beside
    }
}

/// The pane sizes kept for an arrangement.
fn layout(state: &ren_settings::State, arrangement: Arrangement) -> &ren_settings::Layout {
    match arrangement {
        Arrangement::Beside => &state.beside,
        Arrangement::Above => &state.above,
    }
}

fn layout_mut(
    state: &mut ren_settings::State,
    arrangement: Arrangement,
) -> &mut ren_settings::Layout {
    match arrangement {
        Arrangement::Beside => &mut state.beside,
        Arrangement::Above => &mut state.above,
    }
}

/// The tray icon's SVG in a style, at `TRAY_ICON_SIZE`; `dark` is the
/// colour scheme, for the monochrome style that follows it.
fn tray_svg(style: ren_settings::TrayIconStyle, dark: bool) -> String {
    use ren_settings::TrayIconStyle;
    let colour = match style {
        TrayIconStyle::Colour => None,
        TrayIconStyle::Light => Some(TRAY_LIGHT),
        TrayIconStyle::Dark => Some(TRAY_DARK),
        TrayIconStyle::Monochrome if dark => Some(TRAY_LIGHT),
        TrayIconStyle::Monochrome => Some(TRAY_DARK),
    };
    let size = format!("width=\"{TRAY_ICON_SIZE}\" height=\"{TRAY_ICON_SIZE}\"");
    let svg = match colour {
        Some(colour) => TRAY_LOGO_MONO.replace("#000000", colour),
        None => TRAY_LOGO.to_owned(),
    };
    svg.replacen("width=\"22\" height=\"22\"", &size, 1)
}

/// Whether the desktop shows tray icons: a StatusNotifierWatcher with a
/// host is on the session bus (Plasma; GNOME with the AppIndicator
/// extension).
fn tray_host() -> bool {
    let host = || -> zbus::Result<bool> {
        let bus = zbus::blocking::Connection::session()?;
        let watcher = zbus::blocking::Proxy::new(
            &bus,
            "org.kde.StatusNotifierWatcher",
            "/StatusNotifierWatcher",
            "org.kde.StatusNotifierWatcher",
        )?;
        watcher.get_property("IsStatusNotifierHostRegistered")
    };
    host().unwrap_or(false)
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

/// The steps of `--measure-hidden`, `HIDDEN_STEP` apart: the article (and
/// pages) shown, the window hidden, shown again.
const HIDDEN_STEP: Duration = Duration::from_secs(4);

/// Shows the first article, then opens `urls` in tabs, hides the window as
/// in the tray, shows it again and shows the article instead of the page,
/// logging memory use at each step, then quits. After `CYCLE_DELAY`.
fn start_hidden_measurement(app: std::rc::Weak<App>, urls: Vec<String>) -> slint::Timer {
    let timer = slint::Timer::default();
    let started = Instant::now() + CYCLE_DELAY;
    let mut step = 0;
    timer.start(slint::TimerMode::Repeated, HIDDEN_STEP, move || {
        let Some(app) = app.upgrade() else {
            return;
        };
        if Instant::now() < started {
            return;
        }
        // With the helper's, if it runs.
        let log = |when: &str| {
            #[cfg(feature = "servo")]
            pages::log_memory(when, &app.pages.borrow());
            #[cfg(not(feature = "servo"))]
            log_rss(when);
        };
        match step {
            0 => {
                log("at the start");
                if app.reader.borrow().len() > 0 {
                    app.item_clicked(0);
                }
            }
            1 => {
                log("article shown");
                #[cfg(feature = "servo")]
                for url in &urls {
                    if let Err(err) = app.pages.borrow_mut().open(url) {
                        eprintln!("ren: --measure-hidden: {err}");
                    }
                }
                #[cfg(not(feature = "servo"))]
                let _ = &urls;
            }
            2 => {
                log("shown");
                app.hide_window();
            }
            3 => {
                log("hidden");
                app.show_window();
            }
            4 => {
                log("shown again");
                #[cfg(feature = "servo")]
                app.pages.borrow_mut().show_article();
            }
            5 => log("article shown again"),
            _ => {
                let _ = slint::quit_event_loop();
            }
        }
        step += 1;
    });
    timer
}

/// Opens the settings dialog, shows its web page settings (the category
/// with the most rows), closes it, opens About Ren, shows its licenses and
/// closes it, logging
/// memory use at each step, then quits. After `CYCLE_DELAY`, steps
/// `HIDDEN_STEP` apart; freed memory is given back before each step.
fn start_settings_measurement(app: std::rc::Weak<App>) -> slint::Timer {
    let timer = slint::Timer::default();
    let started = Instant::now() + CYCLE_DELAY;
    let mut step = 0;
    timer.start(slint::TimerMode::Repeated, HIDDEN_STEP, move || {
        let Some(app) = app.upgrade() else {
            return;
        };
        if Instant::now() < started {
            return;
        }
        crate::procstat::trim_heap();
        match step {
            0 => {
                log_rss("at the start");
                app.open_settings();
            }
            1 => {
                log_rss("settings open");
                app.with_dialog(|dialog| dialog.select(ren_settings::Category::Pages));
            }
            2 => {
                log_rss("web pages shown");
                app.close_settings();
            }
            3 => {
                log_rss("settings closed");
                app.open_about();
            }
            4 => {
                log_rss("about open");
                if let Some(about) = &*app.about.borrow() {
                    about.invoke_show_licenses();
                    about.set_licenses_shown(true);
                }
            }
            5 => {
                log_rss("licenses shown");
                app.action("about-close");
            }
            6 => log_rss("about closed"),
            _ => {
                let _ = slint::quit_event_loop();
            }
        }
        step += 1;
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
    // Before the database is opened: a Ren that runs already shows it.
    let instance = match claim_instance(options) {
        Some(Claim::Running) => {
            eprintln!("ren: Ren is running already; its window comes to the front");
            return Ok(());
        }
        Some(Claim::First(instance)) => Some(instance),
        None => None,
    };
    // Answered from now on, not only once the window is made: a start
    // that hands over meanwhile would otherwise wait in vain and open a
    // second window.
    let activations = Arc::new(Mutex::new(Activations::default()));
    let socket = instance.map(|mut instance| {
        let target = activations.clone();
        let serving = instance.serve(move |activate| {
            target
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .activate(activate);
        });
        if let Err(err) = serving {
            eprintln!("ren: a second start opens a second window: {err}");
        }
        instance
    });
    let instance = OneInstance {
        socket,
        activations,
    };
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
    // The window's state goes with the default database: the ids in it are
    // that database's, and measurements with other ones start alike.
    let state_path = (demo.is_none() && options.database.is_none())
        .then(|| ren_settings::paths::state(|name| std::env::var(name).ok()))
        .flatten();
    let saved = match state_path {
        Some(path) => Saved::load(path),
        None => (Saved::unsaved(), None),
    };
    // Demo data has neither settings nor a sync.
    let (settings, database) = match demo {
        Some(_) => (None, None),
        None => (
            Some(SettingsFile::open(settings_path(options)?)),
            Some(path),
        ),
    };
    run_window(
        options, started, reader, settings, database, saved, instance,
    )
    .map_err(|err| err.to_string())
}

/// The socket of this Ren, if it has one, and where the starts that hand
/// over to it go.
struct OneInstance {
    // Dropped when the event loop ends: the socket goes with it.
    socket: Option<Instance>,
    activations: Arc<Mutex<Activations>>,
}

/// Where the starts that hand over go: the window once it is made; until
/// then they wait.
#[derive(Default)]
struct Activations {
    window: Option<slint::Weak<MainWindow>>,
    waiting: Vec<Activate>,
}

impl Activations {
    fn activate(&mut self, activate: Activate) {
        match &self.window {
            Some(window) => Self::send(window.clone(), activate),
            None => self.waiting.push(activate),
        }
    }

    /// Sends the starts that waited to `window`, and the next ones.
    fn window_made(&mut self, window: slint::Weak<MainWindow>) {
        for activate in self.waiting.drain(..) {
            Self::send(window.clone(), activate);
        }
        self.window = Some(window);
    }

    /// The window comes to the front once the event loop runs.
    fn send(window: slint::Weak<MainWindow>, activate: Activate) {
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(window) = window.upgrade() {
                window.invoke_activate(activate.token.unwrap_or_default().into());
            }
        });
    }
}

/// Hands over to a Ren that runs already, or claims the socket for this
/// one; `None` where it doesn't take part (`Options::single_instance`)
/// or the socket can't be used.
fn claim_instance(options: &Options) -> Option<Claim> {
    if !options.single_instance() {
        return None;
    }
    let path = ren_settings::paths::instance_socket(|name| std::env::var(name).ok())?;
    match crate::instance::claim(&path, &Activate::from_env()) {
        Ok(claim) => Some(claim),
        Err(err) => {
            eprintln!(
                "ren: {}: {err}; a second start opens a second window",
                path.display()
            );
            None
        }
    }
}

fn run_window(
    options: &Options,
    started: Instant,
    reader: Reader,
    settings: Option<(SettingsFile, Reload)>,
    database: Option<PathBuf>,
    saved: (Saved, Option<String>),
    instance: OneInstance,
) -> Result<(), slint::PlatformError> {
    select_backend(options.backend.as_deref(), options.renderer.as_deref())?;
    slint::set_xdg_app_id(APP_ID)?;
    let (window, app) = build_window(options, started, reader, settings, database, saved)?;
    instance
        .activations
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .window_made(window.as_weak());

    let _autoscroll = options.autoscroll.then(|| start_autoscroll(&window));
    let _measure_sync = options
        .measure_sync
        .then(|| start_sync_measurement(Rc::downgrade(&app)));
    let _measure_settings = options
        .measure_settings
        .then(|| start_settings_measurement(Rc::downgrade(&app)));
    let _measure_hidden = options
        .measure_hidden
        .then(|| start_hidden_measurement(Rc::downgrade(&app), options.tab_urls.clone()));
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
    window.show()?;
    geometry::place(window.as_weak(), app.saved.borrow().state().window.clone());
    let weak = Rc::downgrade(&app);
    geometry::on_moved(window.window(), move || {
        if let Some(app) = weak.upgrade() {
            app.window_moved();
        }
    });
    // Until quit: the window may be hidden in the tray.
    let result = slint::run_event_loop_until_quit();
    // A start from now on opens a new window.
    drop(instance.socket);
    window.hide()?;
    app.dialog.borrow_mut().take();
    app.login.borrow_mut().take();
    app.about.borrow_mut().take();
    app.feed_dialog.borrow_mut().take();
    app.confirm.borrow_mut().take();
    *app.tray.borrow_mut() = None;
    if let Err(err) = app.saved.borrow_mut().save() {
        eprintln!("ren: saving the window's state: {err}");
    }
    // Closing the tabs ends the helper; it saves the site data before it
    // exits, which ren's exit would cut short.
    #[cfg(feature = "servo")]
    {
        app.pages.borrow_mut().close_all();
        crate::tabs::helper::wait_for_helpers();
    }
    result
}

/// The main window and the app behind it, connected, but not shown yet.
fn build_window(
    options: &Options,
    started: Instant,
    reader: Reader,
    settings: Option<(SettingsFile, Reload)>,
    database: Option<PathBuf>,
    (saved, state_problem): (Saved, Option<String>),
) -> Result<(MainWindow, Rc<App>), slint::PlatformError> {
    let window = MainWindow::new()?;
    let dark = options.color_scheme.map(|s| s == cli::ColorScheme::Dark);
    if let Some(dark) = dark {
        window.set_forced_color_scheme(if dark {
            ColorScheme::Dark
        } else {
            ColorScheme::Light
        });
    }
    icons::load(&window, !options.bundled_icons, dark);
    let arrangement = match options.arrangement {
        Some(cli::Arrangement::Beside) => Arrangement::Beside,
        Some(cli::Arrangement::Above) => Arrangement::Above,
        None => settings.as_ref().map_or(Arrangement::Beside, |(file, _)| {
            arrangement_of(file.settings())
        }),
    };
    window.set_arrangement(arrangement);
    geometry::restore(window.window(), &saved.state().window);
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
        options.measure || options.cycle_articles.is_some(),
        move |url| {
            if let Some(window) = weak.upgrade() {
                window.invoke_article_link_action(url.into(), "open".into());
            }
        },
    );

    #[cfg(feature = "servo")]
    let pages = pages::Pages::new(&window, options.measure || options.measure_tabs);

    // Measurements start alike, without syncs of their own, and without a
    // tray icon.
    let measuring = options.measuring();
    let auto_sync = database.is_some() && !measuring;
    let app = Rc::new_cyclic(|this| App {
        this: this.clone(),
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
        no_images: options.no_images,
        saved: RefCell::new(saved),
        arrangement: Cell::new(arrangement),
        arrangement_forced: options.arrangement.is_some(),
        sync_timer: slint::Timer::default(),
        sync_interval: Cell::new(None),
        auto_sync,
        favicons: Favicons::default_dir().map(Favicons::new),
        tray: RefCell::new(None),
        tray_style: Cell::new(ren_settings::TrayIconStyle::Colour),
        close_to_tray: Cell::new(false),
        hidden: Cell::new(false),
        tray_allowed: !measuring,
        geometry_timer: slint::Timer::default(),
        dialog: RefCell::new(None),
        settings_problem: RefCell::new(None),
        settings_watch: RefCell::new(None),
        reload_timer: slint::Timer::default(),
        forced_dark: dark,
        login: RefCell::new(None),
        account_ui: RefCell::default(),
        password_problem: RefCell::new(None),
        synced_once: Cell::new(false),
        sync_failure: RefCell::new(None),
        about: RefCell::new(None),
        confirm: RefCell::new(None),
        feed_changes: RefCell::default(),
        feed_dialog: RefCell::new(None),
        select_after_sync: Cell::new(None),
        full_page_after_sync: Cell::new(None),
        refused_changes: RefCell::default(),
    });
    app.restore();
    match &settings {
        Some((file, _)) => app.apply_settings(file.settings()),
        None => app.apply_settings(&Settings::default()),
    }
    if let Some((file, reload)) = settings {
        if let Some(message) = settings_message(file.path(), &reload) {
            app.status_until_next(message);
        }
        app.note_settings_problem(file.path(), &reload);
        let weak = window.as_weak();
        let saved = move || {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = weak.upgrade() {
                    window.invoke_action("settings-saved".into());
                }
            });
        };
        match ren_settings::watch(file.path(), saved) {
            Ok(watch) => *app.settings_watch.borrow_mut() = Some(watch),
            Err(err) => eprintln!(
                "ren: {}: changes won't apply until the next start: {err}",
                file.path().display()
            ),
        }
        *app.settings.borrow_mut() = Some(file);
        app.refresh_welcome();
    }
    if let Some(problem) = state_problem {
        eprintln!("ren: the window's state: {problem}");
    }
    // The first run: logging in comes first.
    let no_account = app.current_settings().account().is_err();
    if app.auto_sync && no_account {
        let weak = Rc::downgrade(&app);
        slint::Timer::single_shot(FIRST_RUN_DELAY, move || {
            if let Some(app) = weak.upgrade() {
                app.open_account_page();
            }
        });
    } else if app.database.is_some() && app.reader.borrow().tree_rows().len() <= 2 {
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
    window.on_activate(move |token| {
        if let Some(app) = weak.upgrade() {
            app.activate(&token);
        }
    });
    let weak = Rc::downgrade(&app);
    window.on_arrangement_changed(move || {
        if let Some(app) = weak.upgrade() {
            app.arrangement_changed();
        }
    });
    let weak = Rc::downgrade(&app);
    window
        .window()
        .on_close_requested(move || match weak.upgrade() {
            Some(app) => app.close_requested(),
            None => slint::CloseRequestResponse::HideWindow,
        });
    window.set_tabs_available(cfg!(feature = "servo"));
    window.set_version(env!("CARGO_PKG_VERSION").into());
    let weak = Rc::downgrade(&app);
    window.on_search_edited(move |text| {
        if let Some(app) = weak.upgrade() {
            app.search_edited(&text);
        }
    });

    // The first automatic sync, soon after the start.
    if app.sync_interval.get().is_some() {
        let weak = Rc::downgrade(&app);
        slint::Timer::single_shot(FIRST_SYNC_DELAY, move || {
            if let Some(app) = weak.upgrade() {
                app.auto_sync();
            }
        });
    }
    Ok((window, app))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_icons() {
        use ren_settings::TrayIconStyle;
        let colour = tray_svg(TrayIconStyle::Colour, true);
        assert!(colour.contains("url(#petal)"));
        assert!(colour.contains("width=\"44\" height=\"44\""));
        for (style, dark, colour) in [
            (TrayIconStyle::Light, false, TRAY_LIGHT),
            (TrayIconStyle::Dark, true, TRAY_DARK),
            (TrayIconStyle::Monochrome, true, TRAY_LIGHT),
            (TrayIconStyle::Monochrome, false, TRAY_DARK),
        ] {
            let svg = tray_svg(style, dark);
            assert!(svg.contains(&format!("stroke=\"{colour}\"")), "{style:?}");
            assert!(!svg.contains("#000000"));
            assert!(svg.contains("width=\"44\""));
            assert!(slint::Image::load_from_svg_data(svg.as_bytes()).is_ok());
        }
    }

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
