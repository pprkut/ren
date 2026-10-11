// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Input from the window in Servo's terms, smooth wheel scrolling, user
//! gestures, and where pages may navigate to.

use std::time::{Duration, Instant};

use ren_tabs::{ALT, Button, CONTROL, Input, Key, META, Mods, NamedKey, SHIFT};
use servo::{Modifiers, MouseButton};

/// What happens when a page wants to go to a URL with a scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Navigation {
    Allow,
    /// Handed to the desktop (mail links).
    External,
    Deny,
}

/// Web pages, and what pages make themselves (`about:blank` and `srcdoc`
/// frames, `blob:` URLs); no local files (`file:`) or other
/// applications' schemes. Not `data:` either: browsers don't let pages
/// navigate to `data:` URLs, which are used to show phishing pages
/// under an address that names no site. Frames still load them (Servo
/// loads an iframe's `src` without asking), and so do images and styles.
pub fn navigation(scheme: &str) -> Navigation {
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" | "about" | "blob" => Navigation::Allow,
        "mailto" => Navigation::External,
        _ => Navigation::Deny,
    }
}

/// How long after a click or key press a page may open a tab: browsers'
/// transient activation (Servo's `dom_transient_activation_duration_ms`).
pub const GESTURE_TIME: Duration = Duration::from_secs(5);

/// Whether `input` is a user gesture, after which a page may open a tab:
/// a mouse button or a key pressed, except Escape and the modifiers
/// (<https://html.spec.whatwg.org/multipage/interaction.html#activation-triggering-input-event>).
pub fn is_gesture(input: &Input) -> bool {
    match input {
        Input::Down { .. } => true,
        Input::Key {
            key: Key::Named(named),
            down: true,
            ..
        } => !matches!(
            named,
            NamedKey::Escape | NamedKey::Shift | NamedKey::Control | NamedKey::Alt | NamedKey::Meta
        ),
        Input::Key { down, .. } => *down,
        Input::Up { .. } | Input::Move { .. } | Input::Wheel { .. } => false,
    }
}

/// The last user gesture in a tab, which lets its page open one tab.
#[derive(Debug, Default, Clone, Copy)]
pub struct Gesture(Option<Instant>);

impl Gesture {
    pub fn note(&mut self, now: Instant) {
        self.0 = Some(now);
    }

    /// Whether a page may open a tab now; uses the gesture up, so the
    /// next tab needs another one.
    pub fn consume(&mut self, now: Instant) -> bool {
        self.0
            .take()
            .is_some_and(|at| now.saturating_duration_since(at) < GESTURE_TIME)
    }
}

pub fn button(button: Button) -> MouseButton {
    match button {
        Button::Left => MouseButton::Primary,
        Button::Middle => MouseButton::Auxiliary,
        Button::Right => MouseButton::Secondary,
    }
}

pub fn modifiers(mods: Mods) -> Modifiers {
    let mut m = Modifiers::empty();
    m.set(Modifiers::CONTROL, mods & CONTROL != 0);
    m.set(Modifiers::SHIFT, mods & SHIFT != 0);
    m.set(Modifiers::ALT, mods & ALT != 0);
    m.set(Modifiers::META, mods & META != 0);
    m
}

pub fn key(key: &Key) -> servo::Key {
    use servo::NamedKey as S;
    let named = match key {
        Key::Character(text) => return servo::Key::Character(text.clone()),
        Key::Named(named) => named,
    };
    servo::Key::Named(match named {
        NamedKey::Backspace => S::Backspace,
        NamedKey::Tab => S::Tab,
        NamedKey::Enter => S::Enter,
        NamedKey::Escape => S::Escape,
        NamedKey::Delete => S::Delete,
        NamedKey::Shift => S::Shift,
        NamedKey::Control => S::Control,
        NamedKey::Alt => S::Alt,
        NamedKey::Meta => S::Meta,
        NamedKey::ArrowUp => S::ArrowUp,
        NamedKey::ArrowDown => S::ArrowDown,
        NamedKey::ArrowLeft => S::ArrowLeft,
        NamedKey::ArrowRight => S::ArrowRight,
        NamedKey::Home => S::Home,
        NamedKey::End => S::End,
        NamedKey::PageUp => S::PageUp,
        NamedKey::PageDown => S::PageDown,
        NamedKey::Insert => S::Insert,
        NamedKey::F1 => S::F1,
        NamedKey::F2 => S::F2,
        NamedKey::F3 => S::F3,
        NamedKey::F4 => S::F4,
        NamedKey::F5 => S::F5,
        NamedKey::F6 => S::F6,
        NamedKey::F7 => S::F7,
        NamedKey::F8 => S::F8,
        NamedKey::F9 => S::F9,
        NamedKey::F10 => S::F10,
        NamedKey::F11 => S::F11,
        NamedKey::F12 => S::F12,
    })
}

/// Wheel steps at least this long (physical pixels) are spread over a few
/// frames; shorter ones, as from touchpads, are already smooth.
const SMOOTH_MIN: f32 = 40.0;
/// Time between the parts of a wheel step.
pub const SMOOTH_TICK: Duration = Duration::from_millis(16);
/// The part of what is left that each tick scrolls.
const SMOOTH_PART: f32 = 0.35;
/// Below this, the rest is scrolled at once.
const SMOOTH_REST: f32 = 2.0;

/// Servo scrolls each wheel step at once; this spreads long steps over
/// about 100 ms, each tick scrolling a part of what is left.
#[derive(Debug, Default)]
pub struct SmoothScroll {
    /// What is left to scroll, and where.
    pending: Option<Wheel>,
    next_tick: Option<Instant>,
}

/// A wheel delta at a position, as in [`ren_tabs::Input::Wheel`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wheel {
    pub dx: f32,
    pub dy: f32,
    pub x: f32,
    pub y: f32,
}

impl SmoothScroll {
    /// Takes a wheel step; returns what to scroll now if it isn't spread.
    pub fn add(&mut self, wheel: Wheel, now: Instant) -> Option<Wheel> {
        if wheel.dx.abs().max(wheel.dy.abs()) < SMOOTH_MIN {
            // Whatever is pending goes first.
            let rest = self.pending.take().map(|p| Wheel {
                dx: p.dx + wheel.dx,
                dy: p.dy + wheel.dy,
                ..wheel
            });
            self.next_tick = None;
            return Some(rest.unwrap_or(wheel));
        }
        let pending = match self.pending {
            // The same direction adds up; another starts over.
            Some(p) if p.dx * wheel.dx >= 0.0 && p.dy * wheel.dy >= 0.0 => Wheel {
                dx: p.dx + wheel.dx,
                dy: p.dy + wheel.dy,
                ..wheel
            },
            _ => wheel,
        };
        self.pending = Some(pending);
        if self.next_tick.is_none() {
            self.next_tick = Some(now);
        }
        None
    }

    /// When the next part is due, if anything is left.
    pub fn next_tick(&self) -> Option<Instant> {
        self.next_tick
    }

    /// The part to scroll now, if one is due.
    pub fn tick(&mut self, now: Instant) -> Option<Wheel> {
        if self.next_tick.is_none_or(|due| now < due) {
            return None;
        }
        let pending = self.pending?;
        let part = |d: f32| {
            if d.abs() * (1.0 - SMOOTH_PART) < SMOOTH_REST {
                d
            } else {
                d * SMOOTH_PART
            }
        };
        let step = Wheel {
            dx: part(pending.dx),
            dy: part(pending.dy),
            ..pending
        };
        let rest = Wheel {
            dx: pending.dx - step.dx,
            dy: pending.dy - step.dy,
            ..pending
        };
        if rest.dx == 0.0 && rest.dy == 0.0 {
            self.pending = None;
            self.next_tick = None;
        } else {
            self.pending = Some(rest);
            self.next_tick = Some(now + SMOOTH_TICK);
        }
        Some(step)
    }

    /// Forgets what is left, e.g. when another tab is shown.
    pub fn clear(&mut self) {
        self.pending = None;
        self.next_tick = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gestures() {
        let key = |key: Key, down| Input::Key { key, mods: 0, down };
        assert!(is_gesture(&Input::Down {
            button: Button::Left,
            x: 0.0,
            y: 0.0
        }));
        assert!(!is_gesture(&Input::Up {
            button: Button::Left,
            x: 0.0,
            y: 0.0
        }));
        assert!(!is_gesture(&Input::Wheel {
            dx: 0.0,
            dy: 40.0,
            x: 0.0,
            y: 0.0
        }));
        assert!(!is_gesture(&Input::Move { x: 1.0, y: 1.0 }));
        assert!(is_gesture(&key(Key::Character("a".into()), true)));
        assert!(!is_gesture(&key(Key::Character("a".into()), false)));
        assert!(is_gesture(&key(Key::Named(NamedKey::Enter), true)));
        assert!(!is_gesture(&key(Key::Named(NamedKey::Escape), true)));
        assert!(!is_gesture(&key(Key::Named(NamedKey::Control), true)));

        let start = Instant::now();
        let mut gesture = Gesture::default();
        assert!(!gesture.consume(start));
        gesture.note(start);
        assert!(gesture.consume(start + Duration::from_secs(1)));
        assert!(!gesture.consume(start + Duration::from_secs(1)));
        gesture.note(start);
        assert!(!gesture.consume(start + GESTURE_TIME));
    }

    #[test]
    fn navigation_schemes() {
        assert_eq!(navigation("https"), Navigation::Allow);
        assert_eq!(navigation("HTTP"), Navigation::Allow);
        assert_eq!(navigation("about"), Navigation::Allow);
        assert_eq!(navigation("data"), Navigation::Deny);
        assert_eq!(navigation("mailto"), Navigation::External);
        assert_eq!(navigation("file"), Navigation::Deny);
        assert_eq!(navigation("javascript"), Navigation::Deny);
        assert_eq!(navigation("steam"), Navigation::Deny);
    }

    #[test]
    fn modifier_bits() {
        let m = modifiers(CONTROL | ALT);
        assert!(m.contains(Modifiers::CONTROL) && m.contains(Modifiers::ALT));
        assert!(!m.contains(Modifiers::SHIFT) && !m.contains(Modifiers::META));
    }

    fn wheel(dy: f32) -> Wheel {
        Wheel {
            dx: 0.0,
            dy,
            x: 10.0,
            y: 20.0,
        }
    }

    #[test]
    fn short_steps_go_at_once() {
        let mut scroll = SmoothScroll::default();
        let now = Instant::now();
        assert_eq!(scroll.add(wheel(-10.0), now), Some(wheel(-10.0)));
        assert_eq!(scroll.next_tick(), None);
    }

    #[test]
    fn long_steps_are_spread() {
        let mut scroll = SmoothScroll::default();
        let mut now = Instant::now();
        assert_eq!(scroll.add(wheel(-120.0), now), None);
        let mut total = 0.0;
        let mut ticks = 0;
        while let Some(due) = scroll.next_tick() {
            now = due;
            let step = scroll.tick(now).unwrap();
            assert!(step.dy < 0.0 && step.x == 10.0);
            total += step.dy;
            ticks += 1;
            assert!(ticks < 20);
        }
        assert!((total + 120.0).abs() < 0.01, "{total}");
        assert!(ticks > 3, "{ticks} ticks");
        assert_eq!(scroll.tick(now + SMOOTH_TICK), None);
    }

    #[test]
    fn steps_add_up_or_start_over() {
        let mut scroll = SmoothScroll::default();
        let now = Instant::now();
        scroll.add(wheel(-120.0), now);
        scroll.add(wheel(-120.0), now);
        assert_eq!(scroll.tick(now).unwrap().dy, -240.0 * SMOOTH_PART);
        scroll.add(wheel(120.0), now);
        assert_eq!(
            scroll.tick(now + SMOOTH_TICK).unwrap().dy,
            120.0 * SMOOTH_PART
        );
        // A short step takes the rest along.
        let rest = scroll.add(wheel(5.0), now).unwrap();
        assert_eq!(rest.dy, 120.0 * (1.0 - SMOOTH_PART) + 5.0);
        assert_eq!(scroll.next_tick(), None);
    }

    #[test]
    fn not_before_the_tick() {
        let mut scroll = SmoothScroll::default();
        let now = Instant::now();
        scroll.add(wheel(-120.0), now);
        assert!(scroll.tick(now).is_some());
        assert_eq!(scroll.tick(now), None);
        scroll.clear();
        assert_eq!(scroll.tick(now + SMOOTH_TICK), None);
    }
}
