// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Servo and one web view per tab, independent of where frames go.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use dpi::PhysicalSize;
use euclid::Scale;
use ren_tabs::{Cursor, Event, FIRST_PAGE_TAB, Input, Size, TabId};
use servo::{
    CreateNewWebViewRequest, DevicePoint, EventLoopWaker, InputEvent, KeyState, KeyboardEvent,
    LoadStatus, Location, MouseButtonAction, MouseButtonEvent, MouseMoveEvent, NavigationRequest,
    Opts, RenderingContext, Servo, ServoBuilder, Theme, WebView, WebViewBuilder, WebViewDelegate,
    WebViewId, WheelDelta, WheelEvent, WheelMode,
};
use url::Url;

use crate::input::{self, Navigation};

/// The most tabs; pages can't open more.
const MAX_TABS: usize = 50;

pub struct Browser {
    servo: Servo,
    delegate: Rc<Delegate>,
    tabs: Vec<(TabId, WebView)>,
    active: Option<TabId>,
    size: Size,
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
    dark: Cell<bool>,
    size: Cell<Size>,
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

    /// `window.open` and links to new windows open a tab.
    fn request_create_new(&self, parent: WebView, request: CreateNewWebViewRequest) {
        let Some(opener) = self.tab(&parent) else {
            return;
        };
        if self.tab_ids.borrow().len() >= MAX_TABS {
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
        let servo = ServoBuilder::default()
            .opts(opts)
            .event_loop_waker(Box::new(ServoWaker(waker)))
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
            dark: Cell::new(dark),
            size: Cell::new(size),
        });
        Self {
            servo,
            delegate,
            tabs: Vec::new(),
            active: None,
            size,
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

    /// Lets Servo handle its messages; returns what happened.
    pub fn spin(&mut self) -> Vec<Event> {
        self.servo.spin_event_loop();
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
