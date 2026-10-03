// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The article pane with Blitz: an HTML document laid out and painted on
//! the CPU into an RGBA buffer, which the UI shows as an image, plus input
//! handling (scrolling, text selection, links) and the services Blitz asks
//! the embedder for (clipboard, link clicks, fetching images).
//!
//! No UI toolkit types here; `ui` converts events and pixels.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use anyrender::ImageRenderer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use atomic_refcell::AtomicRefCell;
use blitz_dom::{Document, DocumentConfig, FontContext, StyleThreading};
use blitz_html::{HtmlDocument, HtmlProvider};
use blitz_traits::events::{
    BlitzKeyEvent, BlitzPointerEvent, BlitzPointerId, BlitzWheelDelta, BlitzWheelEvent, KeyState,
    MouseEventButton, MouseEventButtons, PointerCoords, PointerDetails, UiEvent,
};
use blitz_traits::navigation::{NavigationOptions, NavigationProvider};
use blitz_traits::net::{NetHandler, NetProvider, Request};
use blitz_traits::shell::{ClipboardError, ColorScheme, ShellProvider, Viewport};
use cursor_icon::CursorIcon;
use keyboard_types::{Code, Key, Location, Modifiers};

const USER_AGENT: &str = concat!("ren/", env!("CARGO_PKG_VERSION"));

/// Largest image (or other resource) fetched for an article.
const RESOURCE_LIMIT: u64 = 20 * 1024 * 1024;

/// Threads fetching and decoding images.
const FETCH_THREADS: usize = 2;

/// Mouse buttons, as far as the article cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerKind {
    Down(Button),
    Up(Button),
    Move,
}

/// Keyboard modifiers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub control: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Mods {
    fn blitz(self) -> Modifiers {
        let mut mods = Modifiers::empty();
        mods.set(Modifiers::CONTROL, self.control);
        mods.set(Modifiers::SHIFT, self.shift);
        mods.set(Modifiers::ALT, self.alt);
        mods.set(Modifiers::META, self.meta);
        mods
    }
}

/// The mouse cursor the article wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    Default,
    Pointer,
    Text,
}

/// Blitz's window services: redraw requests (from any thread, e.g. when an
/// image has loaded) and the clipboard.
struct Shell {
    redraw: AtomicBool,
    /// Wakes the UI so it calls [`HtmlView::render`]; may be called from
    /// any thread.
    wake: Box<dyn Fn() + Send + Sync>,
    clipboard: Mutex<Option<arboard::Clipboard>>,
}

impl Shell {
    fn with_clipboard<T>(
        &self,
        f: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>,
    ) -> Result<T, ClipboardError> {
        let mut clipboard = self.clipboard.lock().map_err(|_| ClipboardError)?;
        if clipboard.is_none() {
            *clipboard = Some(arboard::Clipboard::new().map_err(|err| {
                eprintln!("ren: clipboard: {err}");
                ClipboardError
            })?);
        }
        let clipboard = clipboard.as_mut().ok_or(ClipboardError)?;
        f(clipboard).map_err(|err| {
            eprintln!("ren: clipboard: {err}");
            ClipboardError
        })
    }
}

impl ShellProvider for Shell {
    fn request_redraw(&self) {
        if !self.redraw.swap(true, Ordering::AcqRel) {
            (self.wake)();
        }
    }

    fn get_clipboard_text(&self) -> Result<String, ClipboardError> {
        self.with_clipboard(|clipboard| clipboard.get_text())
    }

    fn set_clipboard_text(&self, text: String) -> Result<(), ClipboardError> {
        // The clipboard keeps serving the text while `Shell` lives.
        self.with_clipboard(|clipboard| clipboard.set_text(text))
    }
}

/// Collects link clicks; the UI decides what to do with them.
#[derive(Default)]
struct Navigation {
    clicks: Mutex<Vec<String>>,
}

impl NavigationProvider for Navigation {
    fn navigate_to(&self, options: NavigationOptions) {
        if let Ok(mut clicks) = self.clicks.lock() {
            clicks.push(options.url.to_string());
        }
    }
}

struct Job {
    doc_id: usize,
    request: Request,
    handler: Box<dyn NetHandler>,
}

/// Fetches images on a few worker threads. Blitz decodes them in the
/// handler, so that happens on the workers too.
struct Net {
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
    /// Fetch http(s) resources; `data:` URLs are always decoded.
    remote: bool,
    /// Requests of older documents (articles no longer shown) are dropped.
    /// Document ids only grow; a newer document may already be fetching
    /// while it is being created, before its id is stored here.
    current_doc: Arc<AtomicUsize>,
}

impl Net {
    fn new(remote: bool) -> Self {
        Self {
            jobs: Mutex::new(None),
            remote,
            current_doc: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn start_workers(&self) -> mpsc::Sender<Job> {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        let agent = http_agent();
        for n in 0..FETCH_THREADS {
            let rx = rx.clone();
            let agent = agent.clone();
            let current_doc = self.current_doc.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("ren-fetch-{n}"))
                .spawn(move || {
                    loop {
                        let job = match rx.lock() {
                            Ok(rx) => match rx.recv() {
                                Ok(job) => job,
                                Err(_) => return,
                            },
                            Err(_) => return,
                        };
                        if job.doc_id < current_doc.load(Ordering::Acquire) {
                            continue;
                        }
                        let url = job.request.url.to_string();
                        match fetch(&agent, &job.request) {
                            Ok(bytes) => job.handler.bytes(url, bytes.into()),
                            Err(err) => eprintln!("ren: {url}: {err}"),
                        }
                    }
                });
            if let Err(err) = spawned {
                eprintln!("ren: cannot start fetch thread: {err}");
            }
        }
        tx
    }
}

impl NetProvider for Net {
    fn fetch(&self, doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        if !self.remote && request.url.scheme() != "data" {
            return;
        }
        let Ok(mut jobs) = self.jobs.lock() else {
            return;
        };
        let tx = jobs.get_or_insert_with(|| self.start_workers());
        let _ = tx.send(Job {
            doc_id,
            request,
            handler,
        });
    }
}

fn http_agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    let tls = TlsConfig::builder()
        .provider(TlsProvider::Rustls)
        .root_certs(RootCerts::PlatformVerifier)
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .user_agent(USER_AGENT)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .new_agent()
}

fn fetch(agent: &ureq::Agent, request: &Request) -> Result<Vec<u8>, String> {
    let url = &request.url;
    match url.scheme() {
        "data" => decode_data_url(url.as_str()),
        "http" | "https" => agent
            .get(url.as_str())
            .call()
            .map_err(|err| err.to_string())?
            .into_body()
            .into_with_config()
            .limit(RESOURCE_LIMIT)
            .read_to_vec()
            .map_err(|err| err.to_string()),
        scheme => Err(format!("unsupported scheme {scheme}")),
    }
}

/// The payload of a `data:` URL.
fn decode_data_url(url: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let rest = url.strip_prefix("data:").ok_or("not a data URL")?;
    let (header, data) = rest.split_once(',').ok_or("data URL without ','")?;
    let data = percent_encoding::percent_decode_str(data).collect::<Vec<u8>>();
    if header.ends_with(";base64") {
        let data: Vec<u8> = data
            .into_iter()
            .filter(|b| !b.is_ascii_whitespace())
            .collect();
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|err| err.to_string())
    } else {
        Ok(data)
    }
}

/// An article rendered by Blitz.
pub struct HtmlView {
    doc: Option<HtmlDocument>,
    renderer: VelloCpuImageRenderer,
    /// Size in physical pixels.
    size: (u32, u32),
    scale: f32,
    color_scheme: ColorScheme,
    font_ctx: FontContext,
    shell: Arc<Shell>,
    navigation: Arc<Navigation>,
    net: Arc<Net>,
    buttons: MouseEventButtons,
    /// Last pointer position, logical pixels relative to the view.
    pointer: (f32, f32),
    started: Instant,
}

impl HtmlView {
    /// `wake` is called, possibly from another thread, when the view needs
    /// to be rendered again, e.g. after an image has loaded. Without
    /// `load_images`, only images in `data:` URLs are shown.
    pub fn new(wake: impl Fn() + Send + Sync + 'static, load_images: bool) -> Self {
        use parley::fontique::{Blob, Collection, CollectionOptions, SourceCache};
        // One font collection for all articles, so the system fonts are
        // scanned only once.
        let mut font_ctx = FontContext {
            source_cache: SourceCache::new_shared(),
            collection: Collection::new(CollectionOptions {
                shared: true,
                system_fonts: true,
            }),
        };
        font_ctx
            .collection
            .register_fonts(Blob::new(Arc::new(blitz_dom::BULLET_FONT) as _), None);

        Self {
            doc: None,
            renderer: VelloCpuImageRenderer::new(1, 1),
            size: (1, 1),
            scale: 1.0,
            color_scheme: ColorScheme::Light,
            font_ctx,
            shell: Arc::new(Shell {
                redraw: AtomicBool::new(false),
                wake: Box::new(wake),
                clipboard: Mutex::new(None),
            }),
            navigation: Arc::new(Navigation::default()),
            net: Arc::new(Net::new(load_images)),
            buttons: MouseEventButtons::None,
            pointer: (0.0, 0.0),
            started: Instant::now(),
        }
    }

    fn viewport(&self) -> Viewport {
        Viewport::new(self.size.0, self.size.1, self.scale, self.color_scheme)
    }

    /// Shows a document; relative URLs resolve against `base_url`.
    pub fn show(&mut self, html: &str, base_url: Option<&str>, dark: bool) {
        self.color_scheme = if dark {
            ColorScheme::Dark
        } else {
            ColorScheme::Light
        };
        let config = DocumentConfig {
            viewport: Some(self.viewport()),
            base_url: base_url.map(str::to_owned),
            net_provider: Some(self.net.clone()),
            navigation_provider: Some(self.navigation.clone()),
            shell_provider: Some(self.shell.clone()),
            html_parser_provider: Some(Arc::new(HtmlProvider)),
            font_ctx: Some(self.font_ctx.clone()),
            // No rayon thread pool for styling one article.
            style_threading: StyleThreading::Sequential,
            ..Default::default()
        };
        // Drop the previous document (and its pending images) first.
        self.doc = None;
        let doc = HtmlDocument::from_html(html, config);
        self.net.current_doc.store(doc.id(), Ordering::Release);
        self.doc = Some(doc);
        self.buttons = MouseEventButtons::None;
        self.shell.request_redraw();
    }

    /// Drops the document, e.g. when no article is selected.
    pub fn clear(&mut self) {
        if let Some(doc) = self.doc.take() {
            // Drop its pending requests.
            self.net.current_doc.store(doc.id() + 1, Ordering::Release);
        }
    }

    /// Sets the size of the view in physical pixels and its scale factor.
    pub fn set_size(&mut self, width: u32, height: u32, scale: f32) {
        let size = (width.max(1), height.max(1));
        if size == self.size && scale == self.scale {
            return;
        }
        self.size = size;
        self.scale = scale;
        self.renderer.resize(size.0, size.1);
        let viewport = self.viewport();
        if let Some(doc) = &mut self.doc {
            doc.set_viewport(viewport);
        }
        self.shell.request_redraw();
    }

    /// Whether something changed since the last [`Self::render`].
    pub fn needs_render(&self) -> bool {
        self.shell.redraw.load(Ordering::Acquire)
    }

    /// Lays out and paints the document into `buffer`: RGBA, premultiplied
    /// alpha, `size()` pixels. Returns the time spent on styling and layout,
    /// and on painting.
    pub fn render(&mut self, buffer: &mut [u8]) -> (Duration, Duration) {
        self.shell.redraw.store(false, Ordering::Release);
        let (width, height) = self.size;
        let scale = f64::from(self.scale);
        let Some(doc) = &mut self.doc else {
            buffer.fill(0);
            return Default::default();
        };
        let started = Instant::now();
        doc.resolve(self.started.elapsed().as_secs_f64());
        let resolved = started.elapsed();
        // The renderer keeps the commands of earlier frames until reset; they
        // would be painted again and may refer to images already freed.
        self.renderer.reset();
        self.renderer.render(
            |scene| blitz_paint::paint_scene(scene, doc, scale, width, height, 0, 0),
            buffer,
        );
        (resolved, started.elapsed() - resolved)
    }

    fn coords(&self, x: f32, y: f32) -> PointerCoords {
        let (scroll_x, scroll_y) = self
            .doc
            .as_ref()
            .map(|doc| {
                let scroll = doc.viewport_scroll();
                (scroll.x as f32, scroll.y as f32)
            })
            .unwrap_or_default();
        PointerCoords {
            page_x: x + scroll_x,
            page_y: y + scroll_y,
            screen_x: x,
            screen_y: y,
            client_x: x,
            client_y: y,
        }
    }

    /// A pointer event at `x`, `y`, in logical pixels relative to the view.
    pub fn pointer(&mut self, kind: PointerKind, x: f32, y: f32, mods: Mods) {
        self.pointer = (x, y);
        let button = match kind {
            PointerKind::Down(b) | PointerKind::Up(b) => match b {
                Button::Left => MouseEventButton::Main,
                Button::Middle => MouseEventButton::Auxiliary,
                Button::Right => MouseEventButton::Secondary,
            },
            PointerKind::Move => MouseEventButton::Main,
        };
        match kind {
            PointerKind::Down(_) => self.buttons |= button.into(),
            PointerKind::Up(_) => self.buttons.remove(button.into()),
            PointerKind::Move => {}
        }
        let event = BlitzPointerEvent {
            id: BlitzPointerId::Mouse,
            is_primary: true,
            coords: self.coords(x, y),
            button,
            buttons: self.buttons,
            mods: mods.blitz(),
            details: PointerDetails::default(),
            element: Default::default(),
            active_pointers: Arc::new(AtomicRefCell::new(Vec::new())),
        };
        let event = match kind {
            PointerKind::Down(_) => UiEvent::PointerDown(event),
            PointerKind::Up(_) => UiEvent::PointerUp(event),
            PointerKind::Move => UiEvent::PointerMove(event),
        };
        self.handle(event);
        if !matches!(kind, PointerKind::Move) {
            self.shell.request_redraw();
        }
    }

    /// Scrolling by `dx`, `dy` logical pixels (positive: towards the top).
    pub fn wheel(&mut self, dx: f32, dy: f32, mods: Mods) {
        let (x, y) = self.pointer;
        let event = BlitzWheelEvent {
            delta: BlitzWheelDelta::Pixels(f64::from(dx), f64::from(dy)),
            coords: self.coords(x, y),
            buttons: self.buttons,
            mods: mods.blitz(),
            element: Default::default(),
        };
        self.handle(UiEvent::Wheel(event));
    }

    /// A key press; `text` is the character for printable keys. Returns
    /// whether the article handled it: only copy (Ctrl+C), which Blitz
    /// handles itself through the shell's clipboard.
    pub fn key(&mut self, text: &str, mods: Mods) -> bool {
        if !((mods.control || mods.meta) && text.eq_ignore_ascii_case("c")) {
            return false;
        }
        let event = BlitzKeyEvent {
            key: Key::Character(text.into()),
            code: Code::KeyC,
            modifiers: mods.blitz(),
            location: Location::Standard,
            is_auto_repeating: false,
            is_composing: false,
            state: KeyState::Pressed,
            text: Some(text.into()),
        };
        self.handle(UiEvent::KeyDown(event));
        true
    }

    /// Copies the selected text to the clipboard. Returns whether there
    /// was a selection.
    pub fn copy(&mut self) -> bool {
        let Some(text) = self.doc.as_ref().and_then(|doc| doc.get_selected_text()) else {
            return false;
        };
        self.shell.set_clipboard_text(text).is_ok()
    }

    fn handle(&mut self, event: UiEvent) {
        if let Some(doc) = &mut self.doc {
            doc.handle_ui_event(event);
        }
    }

    /// The cursor for the current pointer position.
    pub fn cursor(&self) -> Cursor {
        match self.doc.as_ref().and_then(|doc| doc.get_cursor()) {
            Some(CursorIcon::Pointer) => Cursor::Pointer,
            Some(CursorIcon::Text) => Cursor::Text,
            _ => Cursor::Default,
        }
    }

    /// Links clicked since the last call.
    pub fn take_link_clicks(&self) -> Vec<String> {
        self.navigation
            .clicks
            .lock()
            .map(|mut clicks| std::mem::take(&mut *clicks))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_urls() {
        assert_eq!(
            decode_data_url("data:text/plain;base64,aGVs bG8=").unwrap(),
            b"hello"
        );
        assert_eq!(decode_data_url("data:,a%20b").unwrap(), b"a b");
        assert!(decode_data_url("data:nocomma").is_err());
    }
}
