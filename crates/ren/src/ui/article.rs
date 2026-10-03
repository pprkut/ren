// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The article pane: plain text in Slint, or (with `html-view`) the
//! article rendered by Blitz into an image, with input forwarded to it.

use std::cell::RefCell;
use std::rc::Rc;
#[cfg(feature = "html-view")]
use std::time::{Duration, Instant};

use slint::{ComponentHandle, SharedString};

use super::MainWindow;
use crate::article::Article;
#[cfg(feature = "html-view")]
use crate::article::html::{Button, Cursor, HtmlView, Mods, PointerKind};
#[cfg(feature = "html-view")]
use crate::article::{Rgb, Style};

const PLACEHOLDER: &str = "No article selected";

/// Shows articles in the window's article pane.
pub struct ArticlePane {
    window: slint::Weak<MainWindow>,
    article: Option<Article>,
    /// Whether articles are rendered as HTML, and how: `(load_images,
    /// measure)`.
    #[cfg(feature = "html-view")]
    html_mode: Option<(bool, bool)>,
    /// The HTML view, created with the first article so that startup
    /// doesn't pay for Blitz.
    #[cfg(feature = "html-view")]
    html: Option<HtmlPane>,
    /// Called with the URL of a clicked link.
    #[cfg(feature = "html-view")]
    on_link: Box<dyn Fn(&str)>,
}

#[cfg(feature = "html-view")]
struct HtmlPane {
    view: HtmlView,
    /// Two frame buffers: Slint still holds the one shown, the other can be
    /// painted into without a copy.
    buffers: [Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>; 2],
    next: usize,
    cursor: Cursor,
    /// Print timings of each article's first frame.
    measure: bool,
    /// Parse time and start of the article not rendered yet.
    pending_timing: Option<(Duration, Instant)>,
    /// A render is scheduled for after the pending input events.
    render_scheduled: bool,
}

impl ArticlePane {
    /// `html` selects the Blitz view (ignored without `html-view`).
    pub fn new(
        window: &MainWindow,
        html: bool,
        load_images: bool,
        measure: bool,
        on_link: impl Fn(&str) + 'static,
    ) -> Rc<RefCell<Self>> {
        #[cfg(not(feature = "html-view"))]
        let _ = (html, load_images, measure, on_link);

        let pane = Rc::new(RefCell::new(Self {
            window: window.as_weak(),
            article: None,
            #[cfg(feature = "html-view")]
            html_mode: html.then_some((load_images, measure)),
            #[cfg(feature = "html-view")]
            html: None,
            #[cfg(feature = "html-view")]
            on_link: Box::new(on_link),
        }));
        #[cfg(feature = "html-view")]
        connect(window, &pane);
        pane.borrow_mut().clear();
        pane
    }

    fn window(&self) -> MainWindow {
        self.window
            .upgrade()
            .expect("the pane only lives as long as the window")
    }

    pub fn show(&mut self, article: Article) {
        let window = self.window();
        window.set_article_title(article.title.clone().into());
        window.set_article_meta(article.meta.clone().into());
        window.set_article_content_y(0.0);
        #[cfg(feature = "html-view")]
        if self.html_mode.is_some() {
            window.set_article_body(SharedString::new());
            self.article = Some(article);
            self.load_html();
            return;
        }
        window.set_article_body(article.plain_text().into());
        self.article = Some(article);
    }

    pub fn clear(&mut self) {
        self.article = None;
        self.show_placeholder();
        #[cfg(feature = "html-view")]
        if let Some(html) = &mut self.html {
            html.view.clear();
            self.window().set_html_view(false);
        }
    }

    fn show_placeholder(&self) {
        let window = self.window();
        window.set_article_title(PLACEHOLDER.into());
        window.set_article_meta(SharedString::new());
        window.set_article_body(SharedString::new());
    }
}

#[cfg(feature = "html-view")]
impl ArticlePane {
    fn style(window: &MainWindow) -> Style {
        let rgb = |brush: slint::Brush| {
            let c = brush.color();
            Rgb(c.red(), c.green(), c.blue())
        };
        Style {
            background: rgb(window.get_article_background()),
            foreground: rgb(window.get_article_foreground()),
            link: rgb(window.get_article_link()),
            dark: window.get_dark(),
        }
    }

    fn create_html_pane(&self, load_images: bool, measure: bool) -> HtmlPane {
        let weak = self.window.clone();
        HtmlPane {
            view: HtmlView::new(
                move || {
                    let weak = weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(window) = weak.upgrade() {
                            window.invoke_article_render();
                        }
                    });
                },
                load_images,
            ),
            buffers: [None, None],
            next: 0,
            cursor: Cursor::Default,
            measure,
            pending_timing: None,
            render_scheduled: false,
        }
    }

    /// (Re)builds the document of the current article, e.g. after the
    /// colour scheme changed.
    fn load_html(&mut self) {
        let (Some((load_images, measure)), Some(article)) = (self.html_mode, &self.article) else {
            return;
        };
        if self.html.is_none() {
            self.html = Some(self.create_html_pane(load_images, measure));
        }
        let window = self.window();
        let style = Self::style(&window);
        let Some(html) = &mut self.html else {
            return;
        };
        let started = Instant::now();
        let document = article.html_document(&style);
        html.view
            .show(&document, article.url.as_deref(), style.dark);
        html.pending_timing = Some((started.elapsed(), started));
        window.set_html_view(true);
        self.render();
    }

    /// Renders after the input events already queued are handled, so a
    /// burst of events (a high-resolution wheel, fast pointer moves) costs
    /// one frame, not one per event.
    fn schedule_render(&mut self) {
        let Some(html) = &mut self.html else {
            return;
        };
        if html.render_scheduled {
            return;
        }
        html.render_scheduled = true;
        let weak = self.window.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            if let Some(window) = weak.upgrade() {
                window.invoke_article_render();
            }
        });
    }

    /// Renders the HTML view if anything changed.
    fn render(&mut self) {
        let window = self.window();
        let Some(html) = &mut self.html else {
            return;
        };
        html.render_scheduled = false;
        let scale = window.window().scale_factor();
        let width = (window.get_article_width() * scale).round().max(1.0) as u32;
        let height = (window.get_article_height() * scale).round().max(1.0) as u32;
        html.view.set_size(width, height, scale);

        if html.view.needs_render() {
            let slot = html.next;
            let mut buffer = match html.buffers[slot].take() {
                Some(b) if b.width() == width && b.height() == height => b,
                _ => slint::SharedPixelBuffer::new(width, height),
            };
            let (layout, paint) = html.view.render(buffer.make_mut_bytes());
            window.set_article_image(slint::Image::from_rgba8_premultiplied(buffer.clone()));
            html.buffers[slot] = Some(buffer);
            html.next = 1 - slot;

            if let Some((parse, started)) = html.pending_timing.take()
                && html.measure
            {
                eprintln!(
                    "ren: article rendered: parse {:.1} ms, style+layout {:.1} ms, paint {:.1} ms, \
                     total {:.1} ms ({width}x{height})",
                    ms(parse),
                    ms(layout),
                    ms(paint),
                    ms(started.elapsed()),
                );
            }
        }

        let cursor = html.view.cursor();
        if cursor != html.cursor {
            html.cursor = cursor;
            window.set_article_cursor(match cursor {
                Cursor::Default => 0,
                Cursor::Pointer => 1,
                Cursor::Text => 2,
            });
        }

        for url in html.view.take_link_clicks() {
            eprintln!("ren: link clicked: {url}");
            (self.on_link)(&url);
        }
    }

    fn pointer(&mut self, kind: i32, button: i32, x: f32, y: f32, mods: i32) {
        let Some(html) = &mut self.html else {
            return;
        };
        let button = match button {
            1 => Button::Middle,
            2 => Button::Right,
            _ => Button::Left,
        };
        let kind = match kind {
            0 => PointerKind::Down(button),
            1 => PointerKind::Up(button),
            _ => PointerKind::Move,
        };
        html.view.pointer(kind, x, y, decode_mods(mods));
        self.schedule_render();
    }

    fn scroll(&mut self, dx: f32, dy: f32, mods: i32) {
        let Some(html) = &mut self.html else {
            return;
        };
        html.view.wheel(dx, dy, decode_mods(mods));
        self.schedule_render();
    }

    fn key(&mut self, text: &str, mods: i32) -> bool {
        let Some(html) = &mut self.html else {
            return false;
        };
        let handled = html.view.key(text, decode_mods(mods));
        self.schedule_render();
        handled
    }

    fn copy(&mut self) {
        if let Some(html) = &mut self.html {
            html.view.copy();
        }
    }
}

#[cfg(feature = "html-view")]
fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1_000.0
}

#[cfg(feature = "html-view")]
fn decode_mods(mods: i32) -> Mods {
    Mods {
        control: mods & 1 != 0,
        shift: mods & 2 != 0,
        alt: mods & 4 != 0,
        meta: mods & 8 != 0,
    }
}

/// Forwards the pane's callbacks.
#[cfg(feature = "html-view")]
fn connect(window: &MainWindow, pane: &Rc<RefCell<ArticlePane>>) {
    let weak = Rc::downgrade(pane);
    let with = move |f: &dyn Fn(&mut ArticlePane)| {
        if let Some(pane) = weak.upgrade() {
            f(&mut pane.borrow_mut());
        }
    };
    let w = with.clone();
    window.on_article_pointer(move |kind, button, x, y, mods| {
        w(&|p| p.pointer(kind, button, x, y, mods));
    });
    let w = with.clone();
    window.on_article_scroll(move |dx, dy, mods| w(&|p| p.scroll(dx, dy, mods)));
    let weak = Rc::downgrade(pane);
    window.on_article_key(move |text, mods| {
        weak.upgrade()
            .is_some_and(|pane| pane.borrow_mut().key(&text, mods))
    });
    let w = with.clone();
    window.on_article_resized(move || w(&|p| p.render()));
    let w = with.clone();
    window.on_article_render(move || w(&|p| p.render()));
    let w = with.clone();
    window.on_article_theme_changed(move || w(&|p| p.load_html()));
    window.on_article_copy(move || with(&|p| p.copy()));
}
