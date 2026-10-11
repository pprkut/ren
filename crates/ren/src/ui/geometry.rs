// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! The main window's size, position and maximised state, kept in the
//! state file. Sizes are logical pixels, so a change of scale factor keeps
//! them. The position needs winit: Slint doesn't list the screens, and on
//! Wayland, where applications can't place their windows, winit says so
//! instead of returning a position.

use ren_settings::Window;
use slint::winit_030::{WinitWindowAccessor, winit};

use crate::window_state::{Rect, on_screen};

/// Gives the window its last size, position and maximised state, before
/// it is shown.
pub fn restore(window: &slint::Window, saved: &Window) {
    if let (Some(width), Some(height)) = (saved.width, saved.height) {
        window.set_size(slint::LogicalSize::new(width, height));
    }
    if let (Some(x), Some(y)) = (saved.x, saved.y) {
        window.set_position(slint::LogicalPosition::new(x, y));
    }
    if saved.maximized {
        window.set_maximized(true);
    }
}

/// Moves the window onto a screen once it exists, if it was restored to a
/// place that is off the screens now, and shrinks it to fit.
pub fn keep_on_screen(window: slint::Weak<impl slint::ComponentHandle + 'static>) {
    let result = slint::spawn_local(async move {
        let Some(handle) = window.upgrade() else {
            return;
        };
        let Ok(winit_window) = handle.window().winit_window().await else {
            return;
        };
        // Not on Wayland, which doesn't tell where the window is.
        let Ok(position) = winit_window.outer_position() else {
            return;
        };
        let size = winit_window.inner_size();
        let rect = |position: winit::dpi::PhysicalPosition<i32>,
                    size: winit::dpi::PhysicalSize<u32>| Rect {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
        };
        let primary = winit_window.primary_monitor();
        let screens: Vec<Rect> = primary
            .iter()
            .cloned()
            .chain(
                winit_window
                    .available_monitors()
                    .filter(|monitor| Some(monitor) != primary.as_ref()),
            )
            .map(|monitor| rect(monitor.position(), monitor.size()))
            .collect();
        let window = rect(position, size);
        let fitted = on_screen(window, &screens);
        if fitted.width != window.width || fitted.height != window.height {
            let _ = winit_window
                .request_inner_size(winit::dpi::PhysicalSize::new(fitted.width, fitted.height));
        }
        if fitted.x != window.x || fitted.y != window.y {
            winit_window.set_outer_position(winit::dpi::PhysicalPosition::new(fitted.x, fitted.y));
        }
    });
    if let Err(err) = result {
        eprintln!("ren: keeping the window on a screen: {err}");
    }
}

/// The window's size, position and maximised state to keep. While it is
/// maximised (or minimised), its size and position are the ones it had
/// before.
pub fn current(window: &slint::Window, saved: &Window) -> Window {
    let maximized = window.is_maximized();
    if maximized || window.is_minimized() {
        return Window {
            maximized,
            ..saved.clone()
        };
    }
    let scale = window.scale_factor();
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
