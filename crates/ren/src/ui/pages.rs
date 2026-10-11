// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Web pages in tabs next to the article: the tab bar, the page pane and
//! its navigation bar, backed by the Servo helper, which runs only while
//! tabs are open.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use ren_tabs::stats::StageStats;
use ren_tabs::{Button, Cursor, Event, Input, Key, Mods, NamedKey, Size, TabId};
use slint::{ComponentHandle, Model, SharedString, VecModel};

use super::{MainWindow, PageMenu, PageOption, TabRow};
use crate::tabs::Waker;
use crate::tabs::controls::{self, Choice};
use crate::tabs::filters::Filters;
use crate::tabs::helper::{self, Helper};
use crate::tabs::list::{Changes, Tab, TabList};

pub struct Pages {
    window: slint::Weak<MainWindow>,
    helper: Option<Helper>,
    list: TabList,
    rows: Rc<VecModel<TabRow>>,
    /// Where Servo keeps cookies and site data, if it keeps them.
    site_data: Option<PathBuf>,
    keep_site_data: bool,
    /// Where the filter lists for content blocking are kept, the URLs of
    /// those to use (none blocks nothing), and the sites not blocked on.
    filters: Option<Filters>,
    filter_lists: Vec<String>,
    unblocked_sites: Vec<String>,
    /// Lists were downloaded, which the helper should load.
    lists_updated: Arc<AtomicBool>,
    measure: bool,
    /// The addresses of the tabs closed while the window is hidden, and
    /// the one shown.
    suspended: Option<(Vec<String>, Option<usize>)>,
    /// The drop-down and the context menu shown, if they are.
    select: Option<controls::Select>,
    menu: Option<controls::Menu>,
    /// A pump is already queued on the event loop.
    pump_queued: Arc<AtomicBool>,
    stats: StageStats,
}

fn row(tab: &Tab) -> TabRow {
    TabRow {
        title: tab.label().into(),
        loading: tab.loading,
    }
}

impl Pages {
    pub fn new(window: &MainWindow, measure: bool) -> Rc<RefCell<Self>> {
        let rows = Rc::new(VecModel::default());
        window.set_tabs(rows.clone().into());
        let pages = Rc::new(RefCell::new(Self {
            window: window.as_weak(),
            helper: None,
            list: TabList::default(),
            rows,
            site_data: ren_settings::paths::site_data(|name| std::env::var(name).ok()),
            keep_site_data: true,
            filters: ren_settings::paths::filter_lists(|name| std::env::var(name).ok())
                .map(Filters::new),
            // Demo data has no settings file: the defaults.
            filter_lists: ren_settings::Settings::default().filter_lists(),
            unblocked_sites: Vec::new(),
            lists_updated: Arc::default(),
            measure,
            suspended: None,
            select: None,
            menu: None,
            stats: StageStats::new("ui"),
            pump_queued: Arc::default(),
        }));
        connect(window, &pages);
        pages
    }

    fn window(&self) -> MainWindow {
        self.window
            .upgrade()
            .expect("the pages only live as long as the window")
    }

    pub fn helper_pid(&self) -> Option<u32> {
        self.helper.as_ref()?.pid()
    }

    /// Frames shown so far by the current helper.
    pub fn frame_count(&self) -> usize {
        self.helper
            .as_ref()
            .and_then(Helper::frame_stats)
            .map_or(0, |(count, _, _)| count)
    }

    /// Whether any open tab is still loading.
    pub fn loading(&self) -> bool {
        self.list.tabs().iter().any(|tab| tab.loading)
    }

    /// Whether the next helper keeps cookies and site data.
    pub fn set_keep_site_data(&mut self, keep: bool) {
        self.keep_site_data = keep;
    }

    /// Blocks ads and trackers with the filter lists from `lists` (URLs;
    /// none blocks nothing), except on `unblocked` sites.
    pub fn set_blocking(&mut self, lists: Vec<String>, unblocked: Vec<String>) {
        if lists == self.filter_lists && unblocked == self.unblocked_sites {
            return;
        }
        self.filter_lists = lists;
        self.unblocked_sites = unblocked;
        self.show_navigation();
        let blocking = self.blocking();
        if let Some(helper) = &mut self.helper {
            helper.set_blocking(blocking);
            self.update_filters();
        }
    }

    fn blocking(&self) -> Option<ren_tabs::Blocking> {
        self.filters
            .as_ref()?
            .blocking(&self.filter_lists, &self.unblocked_sites)
    }

    /// Downloads the lists that are missing or old; the helper gets them
    /// when they are there.
    fn update_filters(&self) {
        let Some(filters) = &self.filters else {
            return;
        };
        let due = filters.due(&self.filter_lists, SystemTime::now());
        let updated = self.lists_updated.clone();
        let waker = self.waker();
        filters.update(due, move |changed| {
            if changed {
                updated.store(true, Ordering::Release);
                waker();
            }
        });
    }

    fn size(&self) -> Size {
        let window = self.window();
        let scale = window.window().scale_factor();
        Size {
            width: (window.get_page_width() * scale).round().max(1.0) as u32,
            height: (window.get_page_height() * scale).round().max(1.0) as u32,
            scale,
        }
    }

    /// Wakes the UI thread to call `pump`, at most once per pending pump.
    fn waker(&self) -> Waker {
        let weak = self.window.clone();
        let queued = self.pump_queued.clone();
        Arc::new(move || {
            if queued.swap(true, Ordering::AcqRel) {
                return;
            }
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = weak.upgrade() {
                    window.invoke_page_pump();
                }
            });
        })
    }

    /// The helper, started if it isn't running.
    fn helper(&mut self) -> Result<&mut Helper, String> {
        if self.helper.is_none() {
            let window = self.window();
            let profile = self
                .site_data
                .clone()
                .filter(|_| self.keep_site_data)
                .filter(|dir| match private_dir(dir) {
                    Ok(()) => true,
                    Err(err) => {
                        eprintln!("ren: {}: {err}; site data isn't kept", dir.display());
                        false
                    }
                });
            let started = Instant::now();
            let helper = Helper::start(
                self.waker(),
                self.size(),
                window.get_dark(),
                profile,
                self.blocking(),
            )?;
            if self.measure {
                eprintln!(
                    "ren: Servo started in {:.1} ms",
                    started.elapsed().as_secs_f64() * 1_000.0
                );
            }
            self.helper = Some(helper);
            self.update_filters();
        }
        Ok(self.helper.as_mut().expect("started above"))
    }

    /// Opens `url` in a new tab and shows it.
    pub fn open(&mut self, url: &str) -> Result<(), String> {
        let tab = self.helper()?.open(url);
        let index = self.list.open(tab, url);
        self.rows.push(row(&self.list.tabs()[index]));
        self.show_current();
        Ok(())
    }

    /// Shows the page of an article of a feed set to show full pages: in
    /// the tab opened for that, or a new one.
    pub fn open_for_article(&mut self, url: &str) -> Result<(), String> {
        let existing = self
            .list
            .article_tab()
            .and_then(|index| Some((index, self.list.id(index)?)));
        match existing {
            Some((index, tab)) if self.helper.is_some() => {
                self.helper()?.load(tab, url);
                self.list.load(index, url);
                self.update_rows(&[index]);
                self.list.select(Some(index));
                self.show_current();
            }
            _ => {
                self.open(url)?;
                if let Some(index) = self.list.current() {
                    self.list.set_article_tab(index);
                }
            }
        }
        Ok(())
    }

    /// Closes the drop-down and forgets the menu of a page that is no
    /// longer shown; the helper dismisses them.
    fn forget_controls(&mut self) {
        self.menu = None;
        if self.select.take().is_some() {
            self.window().invoke_close_page_select();
        }
    }

    /// Shows the article instead of a page; the tabs stay open.
    pub fn show_article(&mut self) {
        if self.list.select(None) {
            self.show_current();
        }
    }

    /// Shows tab `index`, or the article for -1.
    fn select(&mut self, index: i32) {
        self.list.select(usize::try_from(index).ok());
        self.show_current();
    }

    /// Shows the list's current tab, or the article.
    fn show_current(&mut self) {
        self.forget_controls();
        let window = self.window();
        window.set_current_tab(self.list.current().map_or(-1, |i| i as i32));
        self.show_navigation();
        let tab = self.list.current_tab().map(|t| t.id);
        if let Some(helper) = &mut self.helper {
            helper.activate(tab);
        }
        if tab.is_none() {
            // The page's image isn't needed while the article is shown.
            window.set_page_image(slint::Image::default());
        }
    }

    /// The navigation bar: the current tab's address and history.
    fn show_navigation(&self) {
        let window = self.window();
        let tab = self.list.current_tab();
        window.set_page_url(tab.map_or(SharedString::new(), |t| t.url.as_str().into()));
        window.set_page_can_go_back(tab.is_some_and(|t| t.back));
        window.set_page_can_go_forward(tab.is_some_and(|t| t.forward));
        let blocking = self.blocking();
        let host = self.current_host();
        window.set_page_can_block(blocking.is_some() && host.is_some());
        window.set_page_blocking(
            blocking
                .zip(host)
                .is_some_and(|(blocking, host)| !blocking.unblocks(&host)),
        );
    }

    /// The host of the page shown, if one is.
    pub fn current_host(&self) -> Option<String> {
        let tab = self.list.current_tab()?;
        ren_tabs::host_of(&tab.url).map(str::to_owned)
    }

    pub fn close(&mut self, index: usize) {
        let shown = self.list.current() == Some(index);
        let Some(tab) = self.list.close(index) else {
            return;
        };
        self.rows.remove(index);
        if let Some(helper) = &mut self.helper {
            helper.close(tab);
        }
        if self.list.is_empty() {
            self.stop_helper();
        }
        if shown || self.list.is_empty() {
            self.show_current();
        } else {
            let window = self.window();
            window.set_current_tab(self.list.current().map_or(-1, |i| i as i32));
        }
    }

    /// Closes the tabs while the window is hidden, which ends the helper
    /// and gives Servo's memory back; [`resume`](Self::resume) opens them
    /// again.
    pub fn suspend(&mut self) {
        if self.list.is_empty() {
            return;
        }
        let urls = self.list.tabs().iter().map(|tab| tab.url.clone()).collect();
        self.suspended = Some((urls, self.list.current()));
        self.close_all();
    }

    /// Opens the tabs closed by [`suspend`](Self::suspend) again.
    pub fn resume(&mut self) {
        let Some((urls, current)) = self.suspended.take() else {
            return;
        };
        for url in &urls {
            if let Err(err) = self.open(url) {
                eprintln!("ren: opening the tabs again: {err}");
                return;
            }
        }
        self.list.select(current);
        self.show_current();
    }

    pub fn close_all(&mut self) {
        while !self.list.is_empty() {
            self.close(self.list.len() - 1);
        }
    }

    /// Closes all tabs and removes the cookies and site data Servo kept.
    pub fn clear_site_data(&mut self) -> Result<(), String> {
        self.close_all();
        // The helper writes the site data when it ends.
        helper::wait_for_helpers();
        let Some(dir) = &self.site_data else {
            return Ok(());
        };
        match std::fs::remove_dir_all(dir) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                Err(format!("{}: {err}", dir.display()))
            }
            _ => Ok(()),
        }
    }

    fn stop_helper(&mut self) {
        let Some(helper) = self.helper.take() else {
            return;
        };
        if self.measure
            && let Some((frames, mean, max)) = helper.frame_stats()
        {
            eprintln!(
                "ren: page frames: {frames}, to the UI in {:.2} ms on average, {:.2} ms at most",
                mean.as_secs_f64() * 1_000.0,
                max.as_secs_f64() * 1_000.0
            );
        }
        let started = Instant::now();
        drop(helper);
        if self.measure {
            eprintln!(
                "ren: Servo stopped in {:.1} ms",
                started.elapsed().as_secs_f64() * 1_000.0
            );
        }
        let window = self.window();
        window.set_page_image(slint::Image::default());
        window.set_page_cursor(0);
        // The frame buffers are gone; give the heap's freed memory back.
        crate::procstat::trim_heap();
    }

    fn request_pump(&self) {
        (self.waker())();
    }

    fn pump(&mut self) {
        self.pump_queued.store(false, Ordering::Release);
        let blocking = self
            .lists_updated
            .swap(false, Ordering::AcqRel)
            .then(|| self.blocking());
        let Some(helper) = &mut self.helper else {
            return;
        };
        if let Some(blocking) = blocking {
            helper.set_blocking(blocking);
        }
        let events = helper.pump();
        let frame = helper.take_frame();
        let window = self.window();
        if let Some(frame) = frame {
            self.stats.count("frames");
            window.set_page_image(frame);
        }
        self.stats.report();
        let events = match events {
            Ok(events) => events,
            Err(reason) => {
                eprintln!("ren: {reason}");
                window.set_status_text(reason.into());
                self.helper = None;
                self.list.clear();
                self.rows.clear();
                self.show_current();
                return;
            }
        };
        let mut changes = Changes::default();
        for event in events {
            match event {
                Event::Cursor(cursor) => window.set_page_cursor(match cursor {
                    Cursor::Default => 0,
                    Cursor::Pointer => 1,
                    Cursor::Text => 2,
                }),
                Event::Select {
                    tab,
                    control,
                    area,
                    entries,
                    selected,
                    multiple,
                } => {
                    let select = controls::Select::new(tab, control, entries, selected, multiple);
                    self.show_select(select, area);
                }
                Event::ContextMenu {
                    tab,
                    control,
                    area,
                    entries,
                    link,
                    image,
                } => self.show_menu(
                    controls::Menu::new(tab, control, entries, link, image),
                    area,
                ),
                Event::ControlHidden { control } => {
                    if self.select.as_ref().is_some_and(|s| s.control == control) {
                        self.select = None;
                        window.invoke_close_page_select();
                    }
                    if self.menu.as_ref().is_some_and(|m| m.control == control) {
                        self.menu = None;
                    }
                }
                event => self.list.apply(event, &mut changes),
            }
        }
        self.apply(changes);
    }

    /// Shows a drop-down of the page shown, below the element.
    fn show_select(&mut self, select: controls::Select, area: ren_tabs::Area) {
        if Some(select.tab) != self.current_id() {
            return;
        }
        let window = self.window();
        let scale = self.scale();
        let rows: Vec<PageOption> = select
            .rows()
            .into_iter()
            .map(|row| PageOption {
                label: row.label.into(),
                enabled: row.enabled,
                heading: row.heading,
                selected: row.selected,
            })
            .collect();
        window.set_page_options(Rc::new(VecModel::from(rows)).into());
        window.set_page_select_x(area.x as f32 / scale);
        window.set_page_select_y(area.y as f32 / scale);
        window.set_page_select_width(area.width as f32 / scale);
        window.set_page_select_height(area.height as f32 / scale);
        self.select = Some(select);
        window.invoke_show_page_select();
    }

    /// The reader chose the option in `row` of the drop-down.
    fn select_chosen(&mut self, row: usize) {
        let Some(select) = self.select.take() else {
            return;
        };
        let selected = select.choose(row);
        if let Some(helper) = &mut self.helper {
            helper.answer_select(select.control, selected);
        }
    }

    /// Shows a context menu of the page shown where it was asked for.
    fn show_menu(&mut self, menu: controls::Menu, area: ren_tabs::Area) {
        use ren_tabs::MenuAction;
        if Some(menu.tab) != self.current_id() {
            return;
        }
        let window = self.window();
        let scale = self.scale();
        window.set_page_menu(PageMenu {
            link: menu.link.clone().unwrap_or_default().into(),
            image: menu.image.clone().unwrap_or_default().into(),
            cut: menu.enabled(MenuAction::Cut),
            copy: menu.enabled(MenuAction::Copy),
            paste: menu.enabled(MenuAction::Paste),
            select_all: menu.enabled(MenuAction::SelectAll),
            back: menu.enabled(MenuAction::Back),
            forward: menu.enabled(MenuAction::Forward),
            reload: menu.enabled(MenuAction::Reload),
        });
        self.menu = Some(menu);
        window.invoke_show_page_menu(area.x as f32 / scale, area.y as f32 / scale);
    }

    /// The reader chose an entry of the context menu.
    fn menu_chosen(&mut self, name: &str) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        let chosen = menu.choose(name);
        if let Some(helper) = &mut self.helper {
            helper.answer_menu(menu.control, chosen.as_ref().map(|(_, action)| *action));
        }
        let window = self.window();
        match chosen.map(|(choice, _)| choice) {
            Some(Choice::Copy(url)) => window.invoke_copy_text(url.into()),
            // After this call: opening a tab needs the pages.
            Some(Choice::Open(url)) => link_later(&window, url, "open"),
            Some(Choice::Browser(url)) => link_later(&window, url, "browser"),
            Some(Choice::Page(_)) | None => {}
        }
    }

    /// Shows what events changed.
    fn apply(&mut self, mut changes: Changes) {
        let window = self.window();
        if changes.list {
            self.rows
                .set_vec(self.list.tabs().iter().map(row).collect::<Vec<_>>());
        } else {
            changes.rows.dedup();
            self.update_rows(&changes.rows);
        }
        if let Some(url) = changes.external {
            window.invoke_article_link_action(url.into(), "browser".into());
        }
        if let Some(reason) = changes.crashed {
            window.set_status_text(format!("A page crashed: {reason}").into());
        }
        if self.list.is_empty() {
            self.stop_helper();
            self.show_current();
        } else if changes.current {
            self.show_current();
        } else {
            self.show_navigation();
        }
    }

    fn update_rows(&self, indices: &[usize]) {
        for &index in indices {
            if let Some(tab) = self.list.tabs().get(index)
                && index < self.rows.row_count()
            {
                self.rows.set_row_data(index, row(tab));
            }
        }
    }

    fn resized(&mut self) {
        let size = self.size();
        if let Some(helper) = &mut self.helper {
            helper.resize(size);
            self.request_pump();
        }
    }

    pub fn input(&mut self, input: Input) {
        if let Some(helper) = &mut self.helper {
            helper.input(input);
        }
    }

    fn scale(&self) -> f32 {
        self.window().window().scale_factor()
    }

    /// Pointer input: kind 0 down, 1 up, else a move; button 0 left, 1
    /// middle, 2 right, 3 back, 4 forward.
    fn pointer(&mut self, kind: i32, button: i32, x: f32, y: f32) {
        let scale = self.scale();
        let (x, y) = (x * scale, y * scale);
        let button = match button {
            1 => Button::Middle,
            2 => Button::Right,
            // The mouse's back and forward buttons go through the history.
            3 | 4 => {
                if kind == 1 {
                    self.navigate(if button == 3 { "back" } else { "forward" });
                }
                return;
            }
            _ => Button::Left,
        };
        self.input(match kind {
            0 => Input::Down { button, x, y },
            1 => Input::Up { button, x, y },
            _ => Input::Move { x, y },
        });
    }

    pub fn scroll(&mut self, dx: f32, dy: f32, x: f32, y: f32) {
        let scale = self.scale();
        self.input(Input::Wheel {
            dx: dx * scale,
            dy: dy * scale,
            x: x * scale,
            y: y * scale,
        });
    }

    /// The navigation bar's actions on the current tab: "back",
    /// "forward", "reload".
    pub fn navigate(&mut self, action: &str) {
        let (Some(tab), Some(helper)) = (self.current_id(), self.helper.as_mut()) else {
            return;
        };
        match action {
            "back" => helper.back(tab),
            "forward" => helper.forward(tab),
            "reload" => helper.reload(tab),
            _ => {}
        }
    }

    fn current_id(&self) -> Option<TabId> {
        self.list.current_tab().map(|t| t.id)
    }

    /// The address of the page shown, if one is.
    pub fn current_url(&self) -> Option<String> {
        self.list.current_tab().map(|t| t.url.clone())
    }

    /// Closes the tab shown, if one is.
    pub fn close_current(&mut self) {
        if let Some(index) = self.list.current() {
            self.close(index);
        }
    }

    fn theme_changed(&mut self) {
        let dark = self.window().get_dark();
        if let Some(helper) = &mut self.helper {
            helper.set_dark(dark);
        }
    }
}

/// Opens a link of a page's menu as a link of the article, once the
/// current callback has returned.
fn link_later(window: &MainWindow, url: String, action: &'static str) {
    let weak = window.as_weak();
    slint::Timer::single_shot(Duration::ZERO, move || {
        if let Some(window) = weak.upgrade() {
            window.invoke_article_link_action(url.into(), action.into());
        }
    });
}

/// Creates `dir` readable only by the user, as it holds cookies.
fn private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Maps Slint's key text (characters, or private-use code points for named
/// keys) to a key for the page.
fn key(text: &str) -> Option<Key> {
    use slint::platform::Key as K;
    let named = [
        (K::Backspace, NamedKey::Backspace),
        (K::Tab, NamedKey::Tab),
        (K::Return, NamedKey::Enter),
        (K::Escape, NamedKey::Escape),
        (K::Delete, NamedKey::Delete),
        (K::Shift, NamedKey::Shift),
        (K::Control, NamedKey::Control),
        (K::Alt, NamedKey::Alt),
        (K::Meta, NamedKey::Meta),
        (K::UpArrow, NamedKey::ArrowUp),
        (K::DownArrow, NamedKey::ArrowDown),
        (K::LeftArrow, NamedKey::ArrowLeft),
        (K::RightArrow, NamedKey::ArrowRight),
        (K::Home, NamedKey::Home),
        (K::End, NamedKey::End),
        (K::PageUp, NamedKey::PageUp),
        (K::PageDown, NamedKey::PageDown),
        (K::Insert, NamedKey::Insert),
        (K::F1, NamedKey::F1),
        (K::F2, NamedKey::F2),
        (K::F3, NamedKey::F3),
        (K::F4, NamedKey::F4),
        (K::F5, NamedKey::F5),
        (K::F6, NamedKey::F6),
        (K::F7, NamedKey::F7),
        (K::F8, NamedKey::F8),
        (K::F9, NamedKey::F9),
        (K::F10, NamedKey::F10),
        (K::F11, NamedKey::F11),
        (K::F12, NamedKey::F12),
    ];
    for (slint_key, key) in named {
        if text == SharedString::from(slint_key).as_str() {
            return Some(Key::Named(key));
        }
    }
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        // Other control characters and private-use code points are keys
        // without a mapping.
        (Some(c), None) if !c.is_control() && !('\u{f700}'..='\u{f8ff}').contains(&c) => {
            Some(Key::Character(text.to_owned()))
        }
        _ => None,
    }
}

/// Forwards the window's tab and page callbacks.
fn connect(window: &MainWindow, pages: &Rc<RefCell<Pages>>) {
    let weak = Rc::downgrade(pages);
    let with = move |f: &dyn Fn(&mut Pages)| {
        if let Some(pages) = weak.upgrade() {
            f(&mut pages.borrow_mut());
        }
    };
    let w = with.clone();
    window.on_tab_selected(move |index| w(&|p| p.select(index)));
    let w = with.clone();
    window.on_tab_closed(move |index| w(&|p| p.close(index as usize)));
    let w = with.clone();
    window.on_page_pointer(move |kind, button, x, y, _mods| w(&|p| p.pointer(kind, button, x, y)));
    let w = with.clone();
    window.on_page_scroll(move |dx, dy, x, y| w(&|p| p.scroll(dx, dy, x, y)));
    let w = with.clone();
    window.on_page_key(move |text, mods, down| {
        w(&|p| {
            if let Some(key) = key(&text) {
                p.input(Input::Key {
                    key,
                    mods: mods as Mods,
                    down,
                })
            }
        })
    });
    let w = with.clone();
    window.on_page_navigate(move |action| w(&|p| p.navigate(&action)));
    let w = with.clone();
    window.on_page_resized(move || w(&|p| p.resized()));
    let w = with.clone();
    window.on_page_theme_changed(move || w(&|p| p.theme_changed()));
    let w = with.clone();
    window.on_page_select_chosen(move |row| w(&|p| p.select_chosen(row as usize)));
    let w = with.clone();
    window.on_page_menu_action(move |action| w(&|p| p.menu_chosen(&action)));
    window.on_page_pump(move || with(&|p| p.pump()));
}

/// Time before `--measure-tabs` starts.
const MEASURE_DELAY: Duration = Duration::from_secs(2);
/// The longest wait for pages to load.
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// Time after loading or closing before memory is measured, for Servo's
/// and the allocator's work to settle.
const SETTLE: Duration = Duration::from_secs(3);
/// How long `--measure-tabs` scrolls the page, and by how much per step
/// (logical pixels at about 60 steps per second).
const SCROLL_TIME: Duration = Duration::from_secs(3);
const SCROLL_STEP: f32 = 40.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Start,
    OneTab,
    Scroll,
    ThreeTabs,
    Closed,
    Reopened,
    ClosedAgain,
}

/// Logs the RSS of the UI process and, if it runs, of the helper.
pub(super) fn log_memory(when: &str, pages: &Pages) {
    super::log_rss(when);
    if let Some(pid) = pages.helper_pid() {
        let kib = |field| crate::procstat::proc_status_kib_of(pid, field).unwrap_or(0);
        eprintln!(
            "ren: helper rss {when}: {} KiB (anon {}, file {}, shmem {}; peak {} KiB)",
            kib("VmRSS:"),
            kib("RssAnon:"),
            kib("RssFile:"),
            kib("RssShmem:"),
            kib("VmHWM:")
        );
    }
}

/// `--measure-tabs`: opens one page and then three, scrolls the first,
/// closes them all, opens one again, and logs
/// memory use after each step; then quits.
pub fn start_measurement(pages: std::rc::Weak<RefCell<Pages>>, urls: Vec<String>) -> slint::Timer {
    let timer = slint::Timer::default();
    let url = move |i: usize| urls[i % urls.len()].clone();
    let mut step = Step::Start;
    // When the current step's waiting ends, and whether it waits for
    // loading first.
    let mut wait_until = Instant::now() + MEASURE_DELAY;
    let mut wait_for_load = false;
    let mut scroll_started = (Instant::now(), 0);
    // CPU seconds of the window and the helper when scrolling started.
    let mut scroll_cpu = (None, None);
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(16),
        move || {
            let Some(pages) = pages.upgrade() else {
                return;
            };
            let mut pages = pages.borrow_mut();
            let now = Instant::now();
            if step == Step::Scroll {
                if now < scroll_started.0 + SCROLL_TIME {
                    pages.scroll(0.0, -SCROLL_STEP, 100.0, 100.0);
                    return;
                }
                let frames = pages.frame_count() - scroll_started.1;
                let secs = scroll_started.0.elapsed().as_secs_f64();
                eprintln!("ren: scrolling: {frames} frames in {secs:.1} s");
                let percent = |before: Option<(f64, f64)>, after: Option<(f64, f64)>| {
                    before
                        .zip(after)
                        .map_or("?".to_owned(), |((u0, s0), (u1, s1))| {
                            format!("{:.0}", (u1 + s1 - u0 - s0) / secs * 100.0)
                        })
                };
                eprintln!(
                    "ren: scrolling CPU: window {} %, helper {} %",
                    percent(scroll_cpu.0, crate::procstat::cpu_seconds()),
                    percent(
                        scroll_cpu.1,
                        pages.helper_pid().and_then(crate::procstat::cpu_seconds_of)
                    )
                );
            } else {
                if wait_for_load && pages.loading() && now < wait_until + LOAD_TIMEOUT {
                    return;
                }
                if wait_for_load {
                    if pages.loading() {
                        eprintln!("ren: pages still loading after {LOAD_TIMEOUT:?}");
                    }
                    wait_for_load = false;
                    wait_until = now + SETTLE;
                }
                if now < wait_until {
                    return;
                }
            }
            let open = |pages: &mut Pages, i| {
                if let Err(err) = pages.open(&url(i)) {
                    eprintln!("ren: opening a tab failed: {err}");
                }
            };
            step = match step {
                Step::Start => {
                    log_memory("before tabs", &pages);
                    open(&mut pages, 0);
                    Step::OneTab
                }
                Step::OneTab => {
                    log_memory("with 1 tab", &pages);
                    scroll_started = (now, pages.frame_count());
                    scroll_cpu = (
                        crate::procstat::cpu_seconds(),
                        pages.helper_pid().and_then(crate::procstat::cpu_seconds_of),
                    );
                    Step::Scroll
                }
                Step::Scroll => {
                    open(&mut pages, 1);
                    open(&mut pages, 2);
                    Step::ThreeTabs
                }
                Step::ThreeTabs => {
                    log_memory("with 3 tabs", &pages);
                    pages.close_all();
                    Step::Closed
                }
                Step::Closed => {
                    log_memory("after closing all tabs", &pages);
                    open(&mut pages, 0);
                    Step::Reopened
                }
                Step::Reopened => {
                    log_memory("with 1 tab again", &pages);
                    pages.close_all();
                    Step::ClosedAgain
                }
                Step::ClosedAgain => {
                    log_memory("after closing again", &pages);
                    let _ = slint::quit_event_loop();
                    return;
                }
            };
            wait_for_load = matches!(step, Step::OneTab | Step::ThreeTabs | Step::Reopened);
            wait_until = now + SETTLE;
        },
    );
    timer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_for_the_page() {
        let named = |k: slint::platform::Key| key(SharedString::from(k).as_str());
        assert_eq!(key("a"), Some(Key::Character("a".to_owned())));
        assert_eq!(key("ä"), Some(Key::Character("ä".to_owned())));
        assert_eq!(
            named(slint::platform::Key::Return),
            Some(Key::Named(NamedKey::Enter))
        );
        assert_eq!(
            named(slint::platform::Key::F5),
            Some(Key::Named(NamedKey::F5))
        );
        // Keys without a mapping, and text that isn't one key.
        assert_eq!(named(slint::platform::Key::CapsLock), None);
        assert_eq!(key("ab"), None);
        assert_eq!(key(""), None);
    }
}
