// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Slint glue: binds the view models to the main window.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, VecModel};

use crate::cli::Options;
use crate::dummy::{DummyData, format_date};
use crate::feed_tree::{FeedTree, Node};

slint::include_modules!();

/// The renderer with the smallest footprint, see
/// `docs/decisions/0001-ui-toolkit.md`.
const DEFAULT_RENDERER: &str = "software";

/// Scroll speed of `--autoscroll`, in logical pixels per second.
const AUTOSCROLL_SPEED: f32 = 8_000.0;
/// Time between startup and the start of `--autoscroll`.
const AUTOSCROLL_DELAY: Duration = Duration::from_secs(3);

/// Item list model that builds each row only when the list view asks for
/// it, so only the visible rows ever exist as Slint values.
struct ItemListModel {
    data: Rc<RefCell<DummyData>>,
    ids: RefCell<Vec<u32>>,
    feed_titles: Vec<SharedString>,
    notify: ModelNotify,
}

impl ItemListModel {
    fn id(&self, row: usize) -> Option<u32> {
        self.ids.borrow().get(row).copied()
    }

    fn set_ids(&self, ids: Vec<u32>) {
        *self.ids.borrow_mut() = ids;
        self.notify.reset();
    }
}

impl Model for ItemListModel {
    type Data = ItemRow;

    fn row_count(&self) -> usize {
        self.ids.borrow().len()
    }

    fn row_data(&self, row: usize) -> Option<ItemRow> {
        let item = self.data.borrow().item(self.id(row)?);
        let unread = item.unread();
        Some(ItemRow {
            title: item.title.into(),
            feed: self.feed_titles[item.feed_id as usize].clone(),
            date: format_date(item.pub_date).into(),
            unread,
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
    items: Rc<ItemListModel>,
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
                })
                .collect::<Vec<_>>(),
        );
        self.window()
            .set_current_feed(current.map_or(-1, |i| i as i32));
    }

    fn feed_clicked(&self, row: usize) {
        let Some(node) = self.tree_nodes.borrow().get(row).copied() else {
            return;
        };
        let filter = match node {
            Node::Folder(id) => {
                self.tree.borrow_mut().toggle(id);
                self.refresh_tree();
                return;
            }
            Node::All => None,
            Node::Feed(id) => Some(id),
        };
        self.selected_node.set(node);
        self.window().set_current_feed(row as i32);

        let ids = self.data.borrow().item_ids(filter);
        self.items.set_ids(ids);
        let window = self.window();
        window.set_item_list_content_y(0.0);
        window.set_current_item(-1);
        show_placeholder(&window);
    }

    fn item_clicked(&self, row: usize) {
        let Some(id) = self.items.id(row) else {
            return;
        };
        let window = self.window();
        window.set_current_item(row as i32);

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
    if options.measure {
        log_elapsed(started, "window created");
        report_startup(&window, started);
    }

    let data = Rc::new(RefCell::new(DummyData::new(options.items)));
    let (tree, feed_titles, ids) = {
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
        (tree, titles, data.item_ids(None))
    };

    let items = Rc::new(ItemListModel {
        data: data.clone(),
        ids: RefCell::new(ids),
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
        items,
    });
    app.refresh_tree();

    let weak = Rc::downgrade(&app);
    window.on_feed_clicked(move |row| {
        if let Some(app) = weak.upgrade() {
            app.feed_clicked(row as usize);
        }
    });
    let weak = Rc::downgrade(&app);
    window.on_item_clicked(move |row| {
        if let Some(app) = weak.upgrade() {
            app.item_clicked(row as usize);
        }
    });

    let _autoscroll = options.autoscroll.then(|| start_autoscroll(&window));
    window.run()
}
