// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Servo and one web view per tab, independent of where frames go. Used by
//! the in-process engines and by the helper process.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use dpi::PhysicalSize;
use euclid::Scale;
use servo::{
    Code, DevicePoint, EventLoopWaker, InputEvent, Key, KeyState, KeyboardEvent, LoadStatus,
    Location, Modifiers, MouseButton, MouseButtonAction, MouseButtonEvent, MouseMoveEvent,
    NamedKey, RenderingContext, Servo, ServoBuilder, Theme, WebView, WebViewBuilder,
    WebViewDelegate, WebViewId, WheelDelta, WheelEvent, WheelMode,
};
use url::Url;

use super::{Button, Cursor, Event, Input, Mods, Size, TabId, Waker};

pub struct Browser {
    servo: Servo,
    context: Rc<dyn RenderingContext>,
    delegate: Rc<Delegate>,
    tabs: Vec<(TabId, WebView)>,
    active: Option<TabId>,
    size: Size,
    dark: bool,
}

/// Collects what Servo reports about the web views.
#[derive(Default)]
struct Delegate {
    tab_ids: RefCell<HashMap<WebViewId, TabId>>,
    active: Cell<Option<WebViewId>>,
    frame_ready: Cell<bool>,
    events: RefCell<Vec<Event>>,
}

impl Delegate {
    fn push(&self, webview: &WebView, event: impl FnOnce(TabId) -> Event) {
        if let Some(&tab) = self.tab_ids.borrow().get(&webview.id()) {
            self.events.borrow_mut().push(event(tab));
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

    fn notify_load_status_changed(&self, webview: WebView, status: LoadStatus) {
        let loading = status != LoadStatus::Complete;
        self.push(&webview, |tab| Event::Loading { tab, loading });
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
    /// options are set when it starts and can't be set again.
    pub fn new(waker: Waker, context: Rc<dyn RenderingContext>, size: Size, dark: bool) -> Self {
        // Servo's networking has rustls with two crypto providers, so one
        // must be chosen.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let servo = ServoBuilder::default()
            .event_loop_waker(Box::new(ServoWaker(waker)))
            .build();
        Self {
            servo,
            context,
            delegate: Rc::default(),
            tabs: Vec::new(),
            active: None,
            size,
            dark,
        }
    }

    pub fn open(&mut self, tab: TabId, url: &str) {
        let url = Url::parse(url).unwrap_or_else(|_| Url::parse("about:blank").expect("valid"));
        let webview = WebViewBuilder::new(&self.servo, self.context.clone())
            .url(url)
            .hidpi_scale_factor(Scale::new(self.size.scale))
            .delegate(self.delegate.clone())
            .build();
        webview.notify_theme_change(self.theme());
        self.delegate.tab_ids.borrow_mut().insert(webview.id(), tab);
        self.tabs.push((tab, webview));
        self.activate(tab);
    }

    pub fn close(&mut self, tab: TabId) {
        let Some(index) = self.tabs.iter().position(|(id, _)| *id == tab) else {
            return;
        };
        let (_, webview) = self.tabs.remove(index);
        self.delegate.tab_ids.borrow_mut().remove(&webview.id());
        if self.active == Some(tab) {
            self.active = None;
            self.delegate.active.set(None);
            if let Some(&(next, _)) = self.tabs.get(index.min(self.tabs.len().wrapping_sub(1))) {
                self.activate(next);
            }
        }
    }

    pub fn activate(&mut self, tab: TabId) {
        for (id, webview) in &self.tabs {
            if *id == tab {
                webview.show();
                webview.set_throttled(false);
                webview.focus();
                self.delegate.active.set(Some(webview.id()));
            } else {
                webview.hide();
                webview.set_throttled(true);
            }
        }
        self.active = Some(tab);
        self.delegate.frame_ready.set(true);
    }

    pub fn resize(&mut self, size: Size) {
        if size == self.size {
            return;
        }
        self.size = size;
        for (_, webview) in &self.tabs {
            webview.set_hidpi_scale_factor(Scale::new(size.scale));
        }
        // Servo resizes the shared rendering context and the web views'
        // viewport together, but only if the context's size differs: so
        // resize through a web view, not the context first.
        match self.tabs.first() {
            Some((_, webview)) => webview.resize(physical(size)),
            None => self.context.resize(physical(size)),
        }
    }

    pub fn set_dark(&mut self, dark: bool) {
        self.dark = dark;
        for (_, webview) in &self.tabs {
            webview.notify_theme_change(self.theme());
        }
    }

    fn theme(&self) -> Theme {
        if self.dark { Theme::Dark } else { Theme::Light }
    }

    fn active_webview(&self) -> Option<&WebView> {
        let active = self.active?;
        self.tabs
            .iter()
            .find(|(id, _)| *id == active)
            .map(|(_, w)| w)
    }

    pub fn input(&mut self, input: Input) {
        let Some(webview) = self.active_webview() else {
            return;
        };
        let point = |x: f32, y: f32| DevicePoint::new(x, y).into();
        let button = |b| match b {
            Button::Left => MouseButton::Left,
            Button::Middle => MouseButton::Middle,
            Button::Right => MouseButton::Right,
        };
        let event = match input {
            Input::Down { button: b, x, y } => InputEvent::MouseButton(MouseButtonEvent::new(
                MouseButtonAction::Down,
                button(b),
                point(x, y),
            )),
            Input::Up { button: b, x, y } => InputEvent::MouseButton(MouseButtonEvent::new(
                MouseButtonAction::Up,
                button(b),
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
            Input::Key { text, mods, down } => {
                InputEvent::Keyboard(KeyboardEvent::new_without_event(
                    if down { KeyState::Down } else { KeyState::Up },
                    key(&text),
                    Code::Unidentified,
                    Location::Standard,
                    modifiers(mods),
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
        self.delegate.events.take()
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

/// Maps Slint's key text (characters, or private-use code points for named
/// keys) to a Servo key.
fn key(text: &str) -> Key {
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
    ];
    for (slint_key, servo_key) in named {
        if text == slint::SharedString::from(slint_key).as_str() {
            return Key::Named(servo_key);
        }
    }
    if text.chars().count() == 1 {
        Key::Character(text.to_owned())
    } else {
        Key::Named(NamedKey::Unidentified)
    }
}

fn modifiers(mods: Mods) -> Modifiers {
    let mut m = Modifiers::empty();
    m.set(Modifiers::CONTROL, mods & 1 != 0);
    m.set(Modifiers::SHIFT, mods & 2 != 0);
    m.set(Modifiers::ALT, mods & 4 != 0);
    m.set(Modifiers::META, mods & 8 != 0);
    m
}
