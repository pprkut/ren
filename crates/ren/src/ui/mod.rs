// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Slint glue: binds the view models to the main window.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, VecModel};

use crate::cli::{self, Options};
use crate::dummy::{DummyData, Status, format_date};
use crate::feed_tree::{self, FeedTree, Node};
use crate::item_list::{self, Column, ItemList};

mod icons;

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

/// Item list model that builds each row only when the list view asks for
/// it, so only the visible rows ever exist as Slint values.
struct ItemListModel {
    data: Rc<RefCell<DummyData>>,
    list: Rc<RefCell<ItemList>>,
    feed_titles: Vec<SharedString>,
    notify: ModelNotify,
}

impl Model for ItemListModel {
    type Data = ItemRow;

    fn row_count(&self) -> usize {
        self.list.borrow().ids().len()
    }

    fn row_data(&self, row: usize) -> Option<ItemRow> {
        let item = self.data.borrow().item(self.list.borrow().id(row)?);
        Some(ItemRow {
            title: item.title.into(),
            feed: self.feed_titles[item.feed_id as usize].clone(),
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

struct App {
    window: slint::Weak<MainWindow>,
    data: Rc<RefCell<DummyData>>,
    tree: RefCell<FeedTree>,
    tree_nodes: RefCell<Vec<Node>>,
    selected_node: Cell<Node>,
    feeds: Rc<VecModel<FeedRow>>,
    list: Rc<RefCell<ItemList>>,
    items: Rc<ItemListModel>,
    /// The item shown in the article pane.
    current_item: Cell<Option<u32>>,
    /// Clears the status bar message after `STATUS_TIMEOUT`.
    status_timer: slint::Timer,
}

impl App {
    fn window(&self) -> MainWindow {
        self.window
            .upgrade()
            .expect("callbacks only run while the window exists")
    }

    fn refresh_tree(&self) {
        let rows = self.tree.borrow().rows();
        let selected = self.selected_node.get();
        let current = rows.iter().position(|r| r.node == selected);
        *self.tree_nodes.borrow_mut() = rows.iter().map(|r| r.node).collect();
        self.feeds.set_vec(
            rows.into_iter()
                .map(|r| FeedRow {
                    title: r.title.into(),
                    depth: r.depth.into(),
                    unread: r.unread.try_into().unwrap_or(i32::MAX),
                    is_folder: r.expanded.is_some(),
                    expanded: r.expanded == Some(true),
                    last: r.last,
                    guides: ModelRc::new(VecModel::from(r.guides)),
                })
                .collect::<Vec<_>>(),
        );
        let window = self.window();
        window.set_current_feed(current.map_or(-1, |i| i as i32));
        let total = self.tree.borrow().rows().first().map_or(0, |r| r.unread);
        window.set_unread_text(format!("{total} unread articles").into());
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

    fn not_implemented(&self, action: &str) {
        self.status(format!(
            "Not implemented in the spike: {}",
            action.replace('-', " ")
        ));
    }

    /// Recounts the unread items of all feeds.
    fn refresh_counts(&self) {
        {
            let data = self.data.borrow();
            let mut tree = self.tree.borrow_mut();
            for feed in data.feeds() {
                tree.set_unread(feed.id, data.unread_count(feed.id));
            }
        }
        self.refresh_tree();
    }

    /// Marks an item read or unread and updates its row and the counts.
    fn set_read(&self, id: u32, read: bool) {
        let changed = {
            let mut data = self.data.borrow_mut();
            if read {
                data.mark_read(id)
            } else {
                data.mark_unread(id)
            }
        };
        if !changed {
            return;
        }
        if let Some(row) = self.list.borrow().row_of(id) {
            self.items.notify.row_changed(row);
        }
        let feed_id = self.data.borrow().feed_of(id);
        let unread = self.data.borrow().unread_count(feed_id);
        self.tree.borrow_mut().set_unread(feed_id, unread);
        self.refresh_tree();
    }

    /// Marks all items of a node read.
    fn mark_node_read(&self, node: Node) {
        let feeds = self.tree.borrow().feeds_of(node);
        let count = {
            let mut data = self.data.borrow_mut();
            let ids = data.item_ids(feeds.as_deref());
            ids.into_iter().filter(|&id| data.mark_read(id)).count()
        };
        self.items.notify.reset();
        self.refresh_counts();
        self.status(format!("Marked {count} articles as read"));
    }

    /// Selects the next unread item after the current one.
    fn next_unread(&self) {
        let start = usize::try_from(self.window().get_current_item()).map_or(0, |r| r + 1);
        let next = {
            let data = self.data.borrow();
            let list = self.list.borrow();
            list.ids()
                .iter()
                .skip(start)
                .position(|&id| data.item(id).status != Status::Read)
                .map(|offset| start + offset)
        };
        match next {
            Some(row) => self.item_clicked(row),
            None => self.status("No more unread articles"),
        }
    }

    fn action(&self, name: &str) {
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
            "mark-feed-read" => self.mark_node_read(self.selected_node.get()),
            "mark-read" | "mark-unread" => {
                if let Some(id) = self.current_item.get() {
                    self.set_read(id, name == "mark-read");
                }
            }
            "about" => self.status("ren: a native Nextcloud News reader (spike S1b)"),
            _ => self.not_implemented(name),
        }
    }

    fn feed_menu(&self, row: usize, action: &str) {
        match (action, self.node(row)) {
            ("mark-feed-read", Some(node)) => self.mark_node_read(node),
            _ => self.not_implemented(action),
        }
    }

    fn item_menu(&self, row: usize, action: &str) {
        let Some(id) = self.list.borrow().id(row) else {
            return;
        };
        match action {
            "mark-read" | "mark-unread" => self.set_read(id, action == "mark-read"),
            _ => self.not_implemented(action),
        }
    }

    fn search_edited(&self, text: &str) {
        {
            let data = self.data.borrow();
            self.list.borrow_mut().set_search(text, |id| data.item(id));
        }
        self.items.notify.reset();
        self.keep_current_item();
    }

    /// Selects the current item's row again after the list changed, or
    /// nothing if it is no longer shown.
    fn keep_current_item(&self) {
        let row = self
            .current_item
            .get()
            .and_then(|id| self.list.borrow().row_of(id));
        self.window().set_current_item(row.map_or(-1, |r| r as i32));
    }

    fn node(&self, row: usize) -> Option<Node> {
        self.tree_nodes.borrow().get(row).copied()
    }

    fn feed_clicked(&self, row: usize) {
        if let Some(node) = self.node(row) {
            self.select_node(node);
        }
    }

    fn feed_toggled(&self, row: usize) {
        if let Some(Node::Folder(id)) = self.node(row) {
            self.tree.borrow_mut().toggle(id);
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
        let next = self
            .tree
            .borrow_mut()
            .navigate(self.selected_node.get(), key);
        match next {
            Some(node) => self.select_node(node),
            // Expanding or collapsing a folder.
            None => self.refresh_tree(),
        }
    }

    /// Selects a feed, a folder or "All items" and shows its items.
    fn select_node(&self, node: Node) {
        self.selected_node.set(node);
        self.refresh_tree();

        let feeds = self.tree.borrow().feeds_of(node);
        {
            let data = self.data.borrow();
            let ids = data.item_ids(feeds.as_deref());
            self.list.borrow_mut().set_source(ids, |id| data.item(id));
        }
        self.items.notify.reset();
        self.current_item.set(None);
        let window = self.window();
        window.set_item_list_content_y(0.0);
        window.set_current_item(-1);
        show_placeholder(&window);
    }

    fn sort_clicked(&self, column: usize) {
        if let Some(column) = Column::from_index(column) {
            self.sort_by(column);
        }
    }

    fn sort_by(&self, column: Column) {
        {
            let data = self.data.borrow();
            self.list.borrow_mut().sort_by(column, |id| data.item(id));
        }
        self.items.notify.reset();
        let window = self.window();
        let sort = self.list.borrow().sort();
        window.set_sort_column(sort.column.index() as i32);
        window.set_sort_ascending(sort.ascending);
        // Keep the current item selected, wherever it moved.
        self.keep_current_item();
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
        let len = self.list.borrow().ids().len();
        if let Some(row) = item_list::step(current, key, len, page) {
            self.item_clicked(row);
        }
    }

    fn item_clicked(&self, row: usize) {
        let Some(id) = self.list.borrow().id(row) else {
            return;
        };
        let window = self.window();
        window.set_current_item(row as i32);
        self.current_item.set(Some(id));

        let (item, body, newly_read) = {
            let mut data = self.data.borrow_mut();
            let newly_read = data.mark_read(id);
            (data.item(id), data.body(id), newly_read)
        };
        window.set_article_title(item.title.into());
        window.set_article_meta(
            format!(
                "{} · {}",
                self.items.feed_titles[item.feed_id as usize],
                format_date(item.pub_date)
            )
            .into(),
        );
        window.set_article_body(body.into());
        window.set_article_content_y(0.0);

        if newly_read {
            self.items.notify.row_changed(row);
            let unread = self.data.borrow().unread_count(item.feed_id);
            self.tree.borrow_mut().set_unread(item.feed_id, unread);
            self.refresh_tree();
        }
    }
}

fn show_placeholder(window: &MainWindow) {
    window.set_article_title("No article selected".into());
    window.set_article_meta(SharedString::new());
    window.set_article_body(SharedString::new());
}

/// Uses the given renderer, else the one from `$SLINT_BACKEND`, else
/// `DEFAULT_RENDERER`.
fn select_backend(renderer: Option<&str>) -> Result<(), slint::PlatformError> {
    let mut selector = slint::BackendSelector::new().backend_name("winit".to_owned());
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

pub fn run(options: &Options, started: Instant) -> Result<(), slint::PlatformError> {
    select_backend(options.renderer.as_deref())?;

    let window = MainWindow::new()?;
    icons::load(&window, !options.bundled_icons);
    window.set_arrangement(match options.arrangement {
        cli::Arrangement::Beside => Arrangement::Beside,
        cli::Arrangement::Above => Arrangement::Above,
    });
    if options.measure {
        log_elapsed(started, "window created");
        report_startup(&window, started);
    }

    let data = Rc::new(RefCell::new(DummyData::new(options.items)));
    let list = Rc::new(RefCell::new(ItemList::default()));
    let (tree, feed_titles) = {
        let data = data.borrow();
        let tree = FeedTree::new(
            data.folders(),
            data.feeds()
                .iter()
                .map(|feed| (feed.clone(), data.unread_count(feed.id))),
        );
        let titles = data
            .feeds()
            .iter()
            .map(|f| SharedString::from(f.title.as_str()))
            .collect();
        list.borrow_mut()
            .set_source(data.item_ids(None), |id| data.item(id));
        (tree, titles)
    };

    let items = Rc::new(ItemListModel {
        data: data.clone(),
        list: list.clone(),
        feed_titles,
        notify: ModelNotify::default(),
    });
    let feeds = Rc::new(VecModel::default());
    window.set_items(ModelRc::from(items.clone()));
    window.set_feeds(ModelRc::from(feeds.clone()));
    show_placeholder(&window);

    let app = Rc::new(App {
        window: window.as_weak(),
        data,
        tree: RefCell::new(tree),
        tree_nodes: RefCell::new(Vec::new()),
        selected_node: Cell::new(Node::All),
        feeds,
        list,
        items,
        current_item: Cell::new(None),
        status_timer: slint::Timer::default(),
    });
    app.refresh_tree();

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
    window.on_search_edited(move |text| {
        if let Some(app) = weak.upgrade() {
            app.search_edited(&text);
        }
    });

    let _autoscroll = options.autoscroll.then(|| start_autoscroll(&window));
    window.run()
}
