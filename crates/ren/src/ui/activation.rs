// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bringing the window to the front when Ren is started a second time.
//!
//! - On X11, winit's `focus_window` asks the window manager as an
//!   application, which KWin's focus stealing prevention refuses while
//!   another window is active. Ren asks as a pager does (the task bar,
//!   KWindowSystem's `forceActiveWindow`), which window managers obey: the
//!   user asked for Ren by starting it again.
//! - On Wayland, compositors such as KWin raise a window only with an
//!   activation token, and winit 0.30 can't activate an existing window
//!   with one (its `focus_window` does nothing there). So the launcher's
//!   token goes to the compositor through the xdg-activation protocol on
//!   winit's own Wayland connection.

use slint::winit_030::winit::raw_window_handle::{
    HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
};
use slint::winit_030::winit::window::Window;
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask};
use x11rb::xcb_ffi::XCBConnection;

struct State;

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

wayland_client::delegate_noop!(State: ignore XdgActivationV1);

/// Asks the window manager or compositor to activate `window`, on
/// Wayland with the launcher's `token`. Returns whether the request went
/// out: false on Wayland without a token or xdg-activation. A compositor
/// may still refuse a token it didn't issue or that expired.
pub fn activate(window: &Window, token: &str) -> bool {
    let Ok(handle) = window.window_handle() else {
        return false;
    };
    match handle.as_raw() {
        RawWindowHandle::Xlib(handle) => activate_x11(handle.window as u32),
        RawWindowHandle::Xcb(handle) => activate_x11(handle.window.get()),
        RawWindowHandle::Wayland(_) if !token.is_empty() => activate_wayland(window, token),
        _ => false,
    }
}

/// To the window's virtual desktop (`_NET_CURRENT_DESKTOP`), as pagers
/// do, since not every window manager goes there by itself (Openbox
/// doesn't); then `_NET_ACTIVE_WINDOW` from a pager (source indication
/// 2). On a connection of Ren's own.
fn activate_x11(window: u32) -> bool {
    let Ok((connection, screen)) = XCBConnection::connect(None) else {
        return false;
    };
    let Some(root) = connection
        .setup()
        .roots
        .get(screen)
        .map(|screen| screen.root)
    else {
        return false;
    };
    let atom = |name: &[u8]| {
        connection
            .intern_atom(false, name)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| reply.atom)
    };
    let (Some(active), Some(current), Some(desktop)) = (
        atom(b"_NET_ACTIVE_WINDOW"),
        atom(b"_NET_CURRENT_DESKTOP"),
        atom(b"_NET_WM_DESKTOP"),
    ) else {
        return false;
    };
    let cardinal = |of: u32, property: u32| {
        connection
            .get_property(false, of, property, AtomEnum::CARDINAL, 0, 1)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| reply.value32()?.next())
    };
    let send = |of: u32, kind: u32, data: [u32; 5]| {
        connection
            .send_event(
                false,
                root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                ClientMessageEvent::new(32, of, kind, data),
            )
            .is_ok()
    };
    // 0xFFFFFFFF: on all desktops.
    if let (Some(on), Some(shown)) = (cardinal(window, desktop), cardinal(root, current))
        && on != shown
        && on != u32::MAX
    {
        // The desktop, and the time: now (CurrentTime). Done before the
        // activation (a round trip, as xdotool's `windowactivate` makes):
        // Openbox lost the switch when both came at once.
        send(root, current, [on, 0, 0, 0, 0]);
        let _ = connection
            .get_input_focus()
            .ok()
            .and_then(|cookie| cookie.reply().ok());
    }
    // Source 2: a pager; the time: now; no window active before. Then a
    // round trip, as xdotool's, before the connection closes.
    send(window, active, [2, 0, 0, 0, 0])
        && connection
            .get_input_focus()
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some()
}

fn activate_wayland(window: &Window, token: &str) -> bool {
    let (Ok(display), Ok(handle)) = (window.display_handle(), window.window_handle()) else {
        return false;
    };
    let (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(handle)) =
        (display.as_raw(), handle.as_raw())
    else {
        return false;
    };
    // SAFETY: winit's display stays connected while its window lives, and a
    // backend made from a foreign display doesn't disconnect it when
    // dropped.
    let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
    let connection = Connection::from_backend(backend);
    let Ok((globals, mut queue)) = registry_queue_init::<State>(&connection) else {
        return false;
    };
    let Ok(activation) = globals.bind::<XdgActivationV1, _, _>(&queue.handle(), 1..=1, ()) else {
        return false;
    };
    // SAFETY: the surface is the window's, alive while it is; the proxy
    // only names it in the request.
    let surface =
        unsafe { ObjectId::from_ptr(WlSurface::interface(), handle.surface.as_ptr().cast()) }
            .ok()
            .and_then(|id| WlSurface::from_id(&connection, id).ok());
    let sent = surface.is_some_and(|surface| {
        activation.activate(token.to_owned(), &surface);
        true
    });
    activation.destroy();
    sent && queue.roundtrip(&mut State).is_ok()
}
