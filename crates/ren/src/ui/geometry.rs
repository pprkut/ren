// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The main window's size, position and maximised state, kept in the
//! state file. Sizes are logical pixels, so a change of scale factor keeps
//! them. The position needs winit: Slint doesn't list the screens, and on
//! Wayland, where applications can't place their windows, winit says so
//! instead of returning a position.

use std::time::Duration;

use ren_settings::Window;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};

use crate::window_state::{Rect, on_screen};

/// Gives the window its last size and position before it is shown.
/// [`place`] applies them again, with the maximised state, once the window
/// exists.
pub fn restore(window: &slint::Window, saved: &Window) {
    if let (Some(width), Some(height)) = (saved.width, saved.height) {
        window.set_size(slint::LogicalSize::new(width, height));
    }
    if let (Some(x), Some(y)) = (saved.x, saved.y) {
        window.set_position(slint::LogicalPosition::new(x, y));
    }
}

/// Once the window exists: moves it where it was (kept on a screen, and
/// shrunk to fit one, if the screen it was on is gone), then maximises it
/// if it was. Window managers place a new window before it is theirs, and
/// may drop what it asks for meanwhile: winit asks to maximise it right
/// after mapping, which KWin and Openbox ignore, so it is asked again here.
pub fn place(window: slint::Weak<impl slint::ComponentHandle + 'static>, saved: Window) {
    let result = slint::spawn_local(async move {
        let Some(handle) = window.upgrade() else {
            return;
        };
        let Ok(winit_window) = handle.window().winit_window().await else {
            return;
        };
        let scale = handle.window().scale_factor();
        slint::Timer::single_shot(Duration::ZERO, move || {
            move_onto_screen(&winit_window, &saved, scale);
            if saved.maximized {
                winit_window.set_maximized(true);
            }
        });
    });
    if let Err(err) = result {
        eprintln!("ren: placing the window: {err}");
    }
}

fn rect(position: winit::dpi::PhysicalPosition<i32>, size: winit::dpi::PhysicalSize<u32>) -> Rect {
    Rect {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    }
}

/// The screens, the primary one first.
fn screens(window: &winit::window::Window) -> Vec<Rect> {
    let primary = window.primary_monitor();
    primary
        .iter()
        .cloned()
        .chain(
            window
                .available_monitors()
                .filter(|monitor| Some(monitor) != primary.as_ref()),
        )
        .map(|monitor| rect(monitor.position(), monitor.size()))
        .collect()
}

/// Calls `moved` when the window was moved or resized (by the user or the
/// window manager). Once the window exists.
pub fn on_moved(window: &slint::Window, moved: impl Fn() + 'static) {
    window.on_winit_window_event(move |_, event| {
        if matches!(
            event,
            winit::event::WindowEvent::Moved(_) | winit::event::WindowEvent::Resized(_)
        ) {
            moved();
        }
        EventResult::Propagate
    });
}

/// Moves the window to its saved position, or keeps it where it is, and
/// onto a screen.
fn move_onto_screen(window: &winit::window::Window, saved: &Window, scale: f32) {
    // Not on Wayland, which doesn't tell where the window is.
    let Ok(position) = window.outer_position() else {
        return;
    };
    let size = window.inner_size();
    let current = rect(position, size);
    let wanted = match (saved.x, saved.y) {
        (Some(x), Some(y)) => Rect {
            x: (x * scale).round() as i32,
            y: (y * scale).round() as i32,
            ..current
        },
        _ => current,
    };
    let fitted = on_screen(wanted, &screens(window));
    if (fitted.width, fitted.height) != (current.width, current.height) {
        let _ =
            window.request_inner_size(winit::dpi::PhysicalSize::new(fitted.width, fitted.height));
    }
    if (fitted.x, fitted.y) != (current.x, current.y) {
        window.set_outer_position(winit::dpi::PhysicalPosition::new(fitted.x, fitted.y));
    }
}

/// The window's size, position and maximised state to keep. While it is
/// maximised (or minimised), its size and position are the ones it had
/// before, moved onto the screen it is maximised on, so that it comes
/// back there.
pub fn current(window: &slint::Window, saved: &Window) -> Window {
    let maximized = window.is_maximized();
    let scale = window.scale_factor();
    if maximized || window.is_minimized() {
        let on_its_screen = window
            .with_winit_window(|w| {
                let screen = w.current_monitor()?;
                let (x, y) = (saved.x?, saved.y?);
                let (width, height) = (saved.width?, saved.height?);
                let normal = Rect {
                    x: (x * scale).round() as i32,
                    y: (y * scale).round() as i32,
                    width: (width * scale).round() as u32,
                    height: (height * scale).round() as u32,
                };
                let moved = on_screen(normal, &[rect(screen.position(), screen.size())]);
                Some((moved.x as f32 / scale, moved.y as f32 / scale))
            })
            .flatten();
        let (x, y) = match on_its_screen {
            Some((x, y)) => (Some(x), Some(y)),
            None => (saved.x, saved.y),
        };
        return Window {
            x,
            y,
            maximized,
            ..saved.clone()
        };
    }
    let size = window.size().to_logical(scale);
    let position = window
        .with_winit_window(|w| w.outer_position().ok())
        .flatten()
        .map(|p| (p.x as f32 / scale, p.y as f32 / scale));
    Window {
        width: Some(size.width),
        height: Some(size.height),
        x: position.map(|(x, _)| x),
        y: position.map(|(_, y)| y),
        maximized: false,
    }
}
