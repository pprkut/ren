// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Servo and one web view per tab, independent of where frames go.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use dpi::PhysicalSize;
use euclid::Scale;
use ren_tabs::{
    Blocking, Cursor, DialogAnswer, Event, FIRST_PAGE_TAB, Input, MenuAction, Rgb, Size, TabId,
};
use servo::{
    ColorPicker, ContextMenu, CreateNewWebViewRequest, DevicePoint, EmbedderControl,
    EmbedderControlId, EventLoopWaker, FilePicker, InputEvent, KeyState, KeyboardEvent, LoadStatus,
    Location, MouseButtonAction, MouseButtonEvent, MouseMoveEvent, NavigationRequest, Opts,
    RenderingContext, SelectElement, Servo, ServoBuilder, SimpleDialog, Theme,
    WebRenderDebugOption, WebResourceLoad, WebResourceResponse, WebView, WebViewBuilder,
    WebViewDelegate, WebViewId, WheelDelta, WheelEvent, WheelMode,
};
use url::Url;

use crate::blocking::{Blocker, Verdict};
use crate::controls;
use crate::input::{self, Gesture, Navigation};

/// The most tabs; pages can't open more.
const MAX_TABS: usize = 50;
/// The most tabs pages may have open that they opened themselves.
const MAX_PAGE_TABS: usize = 10;

pub struct Browser {
    servo: Servo,
    waker: Waker,
    delegate: Rc<Delegate>,
    tabs: Vec<(TabId, WebView)>,
    active: Option<TabId>,
    size: Size,
    /// WebRender's profiler is to be shown over the pages
    /// (`REN_SERVO_WR_PROFILER`), from the first tab on: it belongs to
    /// the renderer that all tabs share.
    profiler: bool,
}

/// A control the window shows, until it answers. Dropping one dismisses
/// it (an `alert` counts as answered).
enum Control {
    Select(SelectElement),
    Menu(ContextMenu),
    Dialog(SimpleDialog),
    Colour(ColorPicker),
    Files(FilePicker),
}

/// A [`Control`] with the number the window knows it by, its tab and
/// Servo's id.
struct Shown {
    number: u32,
    tab: TabId,
    id: EmbedderControlId,
    control: Control,
}

/// Called from any thread when Servo has work for the helper's main
/// thread.
pub type Waker = std::sync::Arc<dyn Fn() + Send + Sync>;

/// Collects what Servo reports about the web views.
struct Delegate {
    context: Rc<dyn RenderingContext>,
    tab_ids: RefCell<HashMap<WebViewId, TabId>>,
    active: Cell<Option<WebViewId>>,
    frame_ready: Cell<bool>,
    events: RefCell<Vec<Event>>,
    /// Web views that pages opened, until the browser takes them.
    opened: RefCell<Vec<(TabId, WebView)>>,
    /// Web views that pages closed, until the browser drops them.
    closed: RefCell<Vec<WebViewId>>,
    next_page_tab: Cell<TabId>,
    /// The last user gesture in each tab: pages may open a tab only right
    /// after one.
    gestures: RefCell<HashMap<WebViewId, Gesture>>,
    dark: Cell<bool>,
    size: Cell<Size>,
    blocker: RefCell<Blocker>,
    /// Loads that wait for the filter engine.
    waiting: RefCell<Vec<(WebView, WebResourceLoad)>>,
    /// Drop-downs and context menus the window shows.
    controls: RefCell<Vec<Shown>>,
    next_control: Cell<u32>,
    /// Every request and load, for measurements (`REN_SERVO_REQUEST_LOG`).
    request_log: Option<RequestLog>,
}

/// A line per request and per change of a tab's load status, with the
/// milliseconds since the helper started: to compare what a page loads
/// from one visit to the next (`docs/decisions/0016-performance.md`).
struct RequestLog {
    file: RefCell<std::io::LineWriter<std::fs::File>>,
    started: Instant,
}

impl RequestLog {
    /// From `REN_SERVO_REQUEST_LOG`, a file the lines are added to.
    fn from_env() -> Option<Self> {
        let path = std::env::var_os("REN_SERVO_REQUEST_LOG")?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .inspect_err(|err| eprintln!("ren-servo: the request log: {err}"))
            .ok()?;
        Some(Self {
            file: RefCell::new(std::io::LineWriter::new(file)),
            started: Instant::now(),
        })
    }

    fn line(&self, tab: Option<TabId>, fields: std::fmt::Arguments) {
        use std::io::Write;
        let ms = self.started.elapsed().as_millis();
        let tab = tab.map_or_else(|| "-".to_owned(), |tab| tab.to_string());
        let _ = writeln!(self.file.borrow_mut(), "{ms}\t{tab}\t{fields}");
    }
}

impl Delegate {
    fn tab(&self, webview: &WebView) -> Option<TabId> {
        self.tab_ids.borrow().get(&webview.id()).copied()
    }

    fn push(&self, webview: &WebView, event: impl FnOnce(TabId) -> Event) {
        if let Some(tab) = self.tab(webview) {
            self.events.borrow_mut().push(event(tab));
        }
    }

    /// Takes the control the window answers.
    fn take_control(&self, number: u32) -> Option<Control> {
        let mut controls = self.controls.borrow_mut();
        let index = controls.iter().position(|shown| shown.number == number)?;
        Some(controls.remove(index).control)
    }

    fn theme(&self) -> Theme {
        if self.dark.get() {
            Theme::Dark
        } else {
            Theme::Light
        }
    }
}

impl WebViewDelegate for Delegate {
    fn notify_new_frame_ready(&self, webview: WebView) {
        if self.active.get() == Some(webview.id()) {
            self.frame_ready.set(true);
        }
    }

    fn notify_page_title_changed(&self, webview: WebView, title: Option<String>) {
        if let Some(title) = title {
            self.push(&webview, |tab| Event::Title { tab, title });
        }
    }

    fn notify_url_changed(&self, webview: WebView, url: Url) {
        self.push(&webview, |tab| Event::Url {
            tab,
            url: url.into(),
        });
    }

    fn notify_load_status_changed(&self, webview: WebView, status: LoadStatus) {
        if let Some(log) = &self.request_log {
            let url = webview.url().map(|url| url.to_string()).unwrap_or_default();
            log.line(self.tab(&webview), format_args!("load\t{status:?}\t{url}"));
        }
        let loading = status != LoadStatus::Complete;
        self.push(&webview, |tab| Event::Loading { tab, loading });
    }

    fn notify_history_changed(&self, webview: WebView, entries: Vec<Url>, current: usize) {
        let back = current > 0;
        let forward = current + 1 < entries.len();
        self.push(&webview, |tab| Event::History { tab, back, forward });
    }

    fn notify_cursor_changed(&self, webview: WebView, cursor: servo::Cursor) {
        if self.active.get() == Some(webview.id()) {
            self.events.borrow_mut().push(Event::Cursor(match cursor {
                servo::Cursor::Pointer => Cursor::Pointer,
                servo::Cursor::Text | servo::Cursor::VerticalText => Cursor::Text,
                _ => Cursor::Default,
            }));
        }
    }

    fn notify_closed(&self, webview: WebView) {
        if let Some(tab) = self.tab(&webview) {
            self.events.borrow_mut().push(Event::Closed { tab });
            self.closed.borrow_mut().push(webview.id());
        }
    }

    fn notify_crashed(&self, webview: WebView, reason: String, _backtrace: Option<String>) {
        eprintln!("ren-servo: a page crashed: {reason}");
        self.push(&webview, |tab| Event::Crashed { tab, reason });
    }

    /// Pages only go to web pages: not to local files or other schemes.
    /// Mail links go to the desktop's mail client.
    fn request_navigation(&self, _webview: WebView, request: NavigationRequest) {
        match input::navigation(request.url.scheme()) {
            Navigation::Allow => request.allow(),
            Navigation::External => {
                self.events.borrow_mut().push(Event::External {
                    url: request.url.to_string(),
                });
                request.deny();
            }
            Navigation::Deny => {
                eprintln!("ren-servo: not loading a {} URL", request.url.scheme());
                request.deny();
            }
        }
    }

    /// Ads and trackers aren't loaded (`docs/decisions/0012-content-blocking.md`).
    /// Servo asks for one request at a time and waits for the answer, so
    /// this must not wait for anything.
    fn load_web_resource(&self, webview: WebView, load: WebResourceLoad) {
        let request = load.request();
        let log = |verdict: &str| {
            if let Some(log) = &self.request_log {
                log.line(
                    self.tab(&webview),
                    format_args!(
                        "request\t{verdict}\t{}\t{}",
                        request.destination.as_str(),
                        request.url
                    ),
                );
            }
        };
        // The page the reader opened is always loaded.
        if request.is_for_main_frame {
            log("main");
            return;
        }
        // The tab's URL may still be the last page's while the first
        // requests of the next one come in; the referrer, if the page
        // sends one, is the document that asked.
        let tab = webview.url();
        let verdict = self.blocker.borrow_mut().check(
            &request.url,
            request.referrer_url.as_ref(),
            tab.as_ref(),
            request.destination.as_str(),
        );
        log(match verdict {
            Verdict::Allow => "allowed",
            Verdict::Block => "blocked",
            Verdict::Wait => "waiting",
        });
        match verdict {
            // Dropping the load lets it go on.
            Verdict::Allow => {}
            Verdict::Block => {
                let response = WebResourceResponse::new(request.url.clone());
                load.intercept(response).cancel();
            }
            Verdict::Wait => self.waiting.borrow_mut().push((webview, load)),
        }
    }

    /// Drop-downs, context menus, colour and file pickers and the
    /// `alert`, `confirm` and `prompt` dialogs of the page shown go to the
    /// window. Input methods aren't passed on (0017), and controls of
    /// hidden tabs are dismissed.
    fn show_embedder_control(&self, webview: WebView, control: EmbedderControl) {
        if self.active.get() != Some(webview.id()) {
            return;
        }
        let Some(tab) = self.tab(&webview) else {
            return;
        };
        let number = self.next_control.get();
        let id = control.id();
        let (event, control) = match control {
            EmbedderControl::SelectElement(select) => (
                Event::Select {
                    tab,
                    control: number,
                    area: controls::area(select.position()),
                    entries: controls::select_entries(select.options()),
                    selected: select.selected_options(),
                    multiple: select.allow_select_multiple(),
                },
                Control::Select(select),
            ),
            EmbedderControl::ContextMenu(menu) => {
                let info = menu.element_info();
                (
                    Event::ContextMenu {
                        tab,
                        control: number,
                        area: controls::area(menu.position()),
                        entries: controls::menu_entries(menu.items()),
                        link: info.link_url.as_ref().map(Url::to_string),
                        image: info.image_url.as_ref().map(Url::to_string),
                    },
                    Control::Menu(menu),
                )
            }
            EmbedderControl::SimpleDialog(dialog) => {
                let (kind, message, text) = controls::dialog(&dialog);
                (
                    Event::Dialog {
                        tab,
                        control: number,
                        kind,
                        message,
                        text,
                    },
                    Control::Dialog(dialog),
                )
            }
            EmbedderControl::ColorPicker(picker) => (
                Event::ColourPicker {
                    tab,
                    control: number,
                    area: controls::area(picker.position()),
                    current: picker.current_color().map(controls::rgb),
                },
                Control::Colour(picker),
            ),
            EmbedderControl::FilePicker(picker) => (
                Event::FilePicker {
                    tab,
                    control: number,
                    multiple: picker.allow_select_multiple(),
                    extensions: controls::extensions(picker.filter_patterns()),
                },
                Control::Files(picker),
            ),
            EmbedderControl::InputMethod(_) => return,
        };
        self.next_control.set(number.wrapping_add(1));
        // The window doesn't tell when a menu was closed without a choice:
        // a new control replaces the tab's older ones, which are dismissed.
        let mut controls = self.controls.borrow_mut();
        controls.retain(|shown| shown.tab != tab);
        controls.push(Shown {
            number,
            tab,
            id,
            control,
        });
        self.events.borrow_mut().push(event);
    }

    /// The page took back a control: the window closes it.
    fn hide_embedder_control(&self, _webview: WebView, control_id: EmbedderControlId) {
        let mut controls = self.controls.borrow_mut();
        if let Some(index) = controls.iter().position(|shown| shown.id == control_id) {
            let shown = controls.remove(index);
            self.events.borrow_mut().push(Event::ControlHidden {
                control: shown.number,
            });
        }
    }

    /// `window.open` and links to new windows open a tab, but only right
    /// after a click or key press in the page, once per gesture. Refused
    /// requests are dropped: `window.open` returns null.
    fn request_create_new(&self, parent: WebView, request: CreateNewWebViewRequest) {
        let Some(opener) = self.tab(&parent) else {
            return;
        };
        let gesture = self
            .gestures
            .borrow_mut()
            .get_mut(&parent.id())
            .is_some_and(|gesture| gesture.consume(Instant::now()));
        if !gesture {
            eprintln!("ren-servo: a page opened a tab without a click or key press; not opened");
            return;
        }
        let page_tabs = self
            .tab_ids
            .borrow()
            .values()
            .filter(|&&tab| tab >= FIRST_PAGE_TAB)
            .count();
        if self.tab_ids.borrow().len() >= MAX_TABS || page_tabs >= MAX_PAGE_TABS {
            eprintln!("ren-servo: too many tabs; a page's tab isn't opened");
            return;
        }
        let size = self.size.get();
        let webview = request
            .builder(self.context.clone())
            .hidpi_scale_factor(Scale::new(size.scale))
            .delegate(parent.delegate())
            .build();
        webview.notify_theme_change(self.theme());
        webview.hide();
        let tab = self.next_page_tab.get();
        self.next_page_tab
            .set(tab.wrapping_add(1).max(FIRST_PAGE_TAB));
        self.tab_ids.borrow_mut().insert(webview.id(), tab);
        self.events.borrow_mut().push(Event::Opened { tab, opener });
        self.opened.borrow_mut().push((tab, webview));
    }
}

struct ServoWaker(Waker);

impl EventLoopWaker for ServoWaker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(ServoWaker(self.0.clone()))
    }

    fn wake(&self) {
        (self.0)();
    }
}

fn physical(size: Size) -> PhysicalSize<u32> {
    PhysicalSize::new(size.width.max(1), size.height.max(1))
}

impl Browser {
    /// Starts Servo. It can only be started once per process: its global
    /// options are set when it starts and can't be set again. With a
    /// profile directory, cookies and site data are kept there.
    pub fn new(
        waker: Waker,
        context: Rc<dyn RenderingContext>,
        size: Size,
        dark: bool,
        profile: Option<PathBuf>,
        blocking: Option<Blocking>,
    ) -> Self {
        // Servo's networking has rustls with two crypto providers, so one
        // must be chosen.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let opts = Opts {
            // A crashing page ends its own tab, not the helper.
            hard_fail: false,
            temporary_storage: profile.is_none(),
            config_dir: profile,
            ..Opts::default()
        };
        let mut blocker = Blocker::default();
        blocker.configure(blocking, &waker);
        let servo = ServoBuilder::default()
            .opts(opts)
            .event_loop_waker(Box::new(ServoWaker(waker.clone())))
            .build();
        let delegate = Rc::new(Delegate {
            context,
            tab_ids: RefCell::default(),
            active: Cell::new(None),
            frame_ready: Cell::new(false),
            events: RefCell::default(),
            opened: RefCell::default(),
            closed: RefCell::default(),
            next_page_tab: Cell::new(FIRST_PAGE_TAB),
            gestures: RefCell::default(),
            dark: Cell::new(dark),
            size: Cell::new(size),
            blocker: RefCell::new(blocker),
            waiting: RefCell::default(),
            controls: RefCell::default(),
            next_control: Cell::new(0),
            request_log: RequestLog::from_env(),
        });
        Self {
            servo,
            waker,
            delegate,
            tabs: Vec::new(),
            active: None,
            size,
            profiler: std::env::var_os("REN_SERVO_WR_PROFILER").is_some(),
        }
    }

    pub fn open(&mut self, tab: TabId, url: &str) {
        if self.tabs.len() >= MAX_TABS {
            self.delegate
                .events
                .borrow_mut()
                .push(Event::Closed { tab });
            return;
        }
        let webview = WebViewBuilder::new(&self.servo, self.delegate.context.clone())
            .url(web_url(url))
            .hidpi_scale_factor(Scale::new(self.size.scale))
            .delegate(self.delegate.clone())
            .build();
        webview.notify_theme_change(self.delegate.theme());
        webview.hide();
        if std::mem::take(&mut self.profiler) {
            webview.toggle_webrender_debugging(WebRenderDebugOption::Profiler);
        }
        self.delegate.tab_ids.borrow_mut().insert(webview.id(), tab);
        self.tabs.push((tab, webview));
    }

    fn webview(&self, tab: TabId) -> Option<&WebView> {
        self.tabs.iter().find(|(id, _)| *id == tab).map(|(_, w)| w)
    }

    pub fn load(&mut self, tab: TabId, url: &str) {
        if let Some(webview) = self.webview(tab) {
            webview.load(web_url(url));
        }
    }

    pub fn back(&mut self, tab: TabId) {
        if let Some(webview) = self.webview(tab).filter(|w| w.can_go_back()) {
            webview.go_back(1);
        }
    }

    pub fn forward(&mut self, tab: TabId) {
        if let Some(webview) = self.webview(tab).filter(|w| w.can_go_forward()) {
            webview.go_forward(1);
        }
    }

    pub fn reload(&mut self, tab: TabId) {
        if let Some(webview) = self.webview(tab) {
            webview.reload();
        }
    }

    pub fn close(&mut self, tab: TabId) {
        let Some(index) = self.tabs.iter().position(|(id, _)| *id == tab) else {
            return;
        };
        let (_, webview) = self.tabs.remove(index);
        self.delegate.tab_ids.borrow_mut().remove(&webview.id());
        self.delegate.gestures.borrow_mut().remove(&webview.id());
        self.dismiss_controls(|shown| shown == tab);
        // The window activates another tab if it wants one.
        if self.active == Some(tab) {
            self.active = None;
            self.delegate.active.set(None);
        }
    }

    /// Shows `tab`, or no page for `None`; the others are hidden.
    pub fn activate(&mut self, tab: Option<TabId>) {
        self.delegate.active.set(None);
        for (id, webview) in &self.tabs {
            if Some(*id) == tab {
                webview.show();
                webview.set_focused(true);
                // A page opened while hidden may have another size.
                webview.resize(physical(self.size));
                self.delegate.active.set(Some(webview.id()));
            } else {
                webview.set_focused(false);
                webview.hide();
            }
        }
        self.active = tab;
        self.delegate.frame_ready.set(tab.is_some());
        // The window closed those of the tab it hid.
        self.dismiss_controls(|shown| Some(shown) != tab);
    }

    /// Dismisses the drop-downs and menus of the tabs `which` picks.
    fn dismiss_controls(&self, which: impl Fn(TabId) -> bool) {
        self.delegate
            .controls
            .borrow_mut()
            .retain(|shown| !which(shown.tab));
    }

    /// The window's answer to a drop-down: the options chosen, or none to
    /// leave it as it was.
    pub fn answer_select(&mut self, number: u32, selected: Option<Vec<usize>>) {
        if let Some(Control::Select(mut select)) = self.delegate.take_control(number) {
            if let Some(selected) = selected {
                select.select(selected);
            }
            select.submit();
        }
    }

    /// The window's answer to a context menu: the entry chosen, or none.
    pub fn answer_menu(&mut self, number: u32, action: Option<MenuAction>) {
        if let Some(Control::Menu(menu)) = self.delegate.take_control(number) {
            match action.and_then(controls::servo_action) {
                Some(action) => menu.select(action),
                None => menu.dismiss(),
            }
        }
    }

    /// The window's answer to a page's `alert`, `confirm` or `prompt`.
    pub fn answer_dialog(&mut self, number: u32, answer: DialogAnswer) {
        let Some(Control::Dialog(dialog)) = self.delegate.take_control(number) else {
            return;
        };
        match (dialog, answer) {
            (SimpleDialog::Prompt(mut prompt), DialogAnswer::Ok(text)) => {
                if let Some(text) = text {
                    prompt.set_current_value(&text);
                }
                prompt.confirm();
            }
            (dialog, DialogAnswer::Ok(_)) => dialog.confirm(),
            (dialog, DialogAnswer::Cancel) => dialog.dismiss(),
        }
    }

    /// The window's answer to a colour picker: the colour chosen, or none
    /// to keep the page's.
    pub fn answer_colour(&mut self, number: u32, colour: Option<Rgb>) {
        if let Some(Control::Colour(mut picker)) = self.delegate.take_control(number) {
            if let Some(colour) = colour {
                picker.select(Some(controls::servo_rgb(colour)));
            }
            picker.submit();
        }
    }

    /// The window's answer to a file picker: the files chosen, or none.
    pub fn answer_files(&mut self, number: u32, paths: Option<Vec<PathBuf>>) {
        if let Some(Control::Files(mut picker)) = self.delegate.take_control(number) {
            match paths {
                Some(paths) => {
                    picker.select(&paths);
                    picker.submit();
                }
                None => picker.dismiss(),
            }
        }
    }

    pub fn resize(&mut self, size: Size) {
        if size == self.size {
            return;
        }
        self.size = size;
        self.delegate.size.set(size);
        for (_, webview) in &self.tabs {
            webview.set_hidpi_scale_factor(Scale::new(size.scale));
        }
        // Servo resizes the shared rendering context and the web views'
        // viewport together, but only if the context's size differs: so
        // resize through a web view, not the context first.
        match self.tabs.first() {
            Some((_, webview)) => webview.resize(physical(size)),
            None => self.delegate.context.resize(physical(size)),
        }
    }

    pub fn set_dark(&mut self, dark: bool) {
        self.delegate.dark.set(dark);
        for (_, webview) in &self.tabs {
            webview.notify_theme_change(self.delegate.theme());
        }
    }

    fn active_webview(&self) -> Option<&WebView> {
        self.webview(self.active?)
    }

    pub fn input(&mut self, input: Input) {
        let Some(webview) = self.active_webview() else {
            return;
        };
        if input::is_gesture(&input) {
            self.delegate
                .gestures
                .borrow_mut()
                .entry(webview.id())
                .or_default()
                .note(Instant::now());
        }
        let point = |x: f32, y: f32| DevicePoint::new(x, y).into();
        let event = match input {
            Input::Down { button, x, y } => InputEvent::MouseButton(MouseButtonEvent::new(
                MouseButtonAction::Down,
                input::button(button),
                point(x, y),
            )),
            Input::Up { button, x, y } => InputEvent::MouseButton(MouseButtonEvent::new(
                MouseButtonAction::Up,
                input::button(button),
                point(x, y),
            )),
            Input::Move { x, y } => InputEvent::MouseMove(MouseMoveEvent::new(point(x, y))),
            Input::Wheel { dx, dy, x, y } => InputEvent::Wheel(WheelEvent::new(
                WheelDelta {
                    x: dx.into(),
                    y: dy.into(),
                    z: 0.0,
                    mode: WheelMode::DeltaPixel,
                },
                point(x, y),
            )),
            Input::Key { key, mods, down } => {
                InputEvent::Keyboard(KeyboardEvent::new_without_event(
                    if down { KeyState::Down } else { KeyState::Up },
                    input::key(&key),
                    servo::Code::Unidentified,
                    Location::Standard,
                    input::modifiers(mods),
                    false,
                    false,
                ))
            }
        };
        webview.notify_input_event(event);
    }

    /// Blocks content with `blocking` from now on.
    pub fn set_blocking(&mut self, blocking: Option<Blocking>) {
        self.delegate
            .blocker
            .borrow_mut()
            .configure(blocking, &self.waker);
        self.check_waiting();
    }

    /// The requests checked and blocked since the last call.
    pub fn take_block_counts(&mut self) -> (u32, u32) {
        self.delegate.blocker.borrow_mut().take_counts()
    }

    /// Checks the loads that waited for the filter engine, once it's
    /// there (or failed to load).
    fn check_waiting(&self) {
        let ready = {
            let mut blocker = self.delegate.blocker.borrow_mut();
            blocker.poll();
            blocker.is_ready()
        };
        if ready {
            for (webview, load) in self.delegate.waiting.take() {
                self.delegate.load_web_resource(webview, load);
            }
        }
    }

    /// Lets Servo handle its messages; returns what happened.
    pub fn spin(&mut self) -> Vec<Event> {
        self.servo.spin_event_loop();
        self.check_waiting();
        self.tabs.append(&mut self.delegate.opened.borrow_mut());
        for id in self.delegate.closed.take() {
            let tab = self.delegate.tab_ids.borrow().get(&id).copied();
            if let Some(tab) = tab {
                self.close(tab);
            }
        }
        self.delegate.events.take()
    }

    /// Paints the active tab again with the next `paint`.
    pub fn repaint(&mut self) {
        self.delegate.frame_ready.set(self.active.is_some());
    }

    /// The active tab, for frames.
    pub fn active(&self) -> Option<TabId> {
        self.active
    }

    /// Paints the active tab into the rendering context if it has a new
    /// frame; returns whether it did.
    pub fn paint(&mut self) -> bool {
        if !self.delegate.frame_ready.take() {
            return false;
        }
        let Some(webview) = self.active_webview() else {
            return false;
        };
        webview.paint();
        true
    }
}

/// The URL to load: only web pages, else a blank page.
fn web_url(url: &str) -> Url {
    Url::parse(url)
        .ok()
        .filter(|url| input::navigation(url.scheme()) == Navigation::Allow)
        .unwrap_or_else(|| Url::parse("about:blank").expect("valid"))
}
