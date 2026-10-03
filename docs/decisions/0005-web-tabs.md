<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0005 — Web pages in Servo tabs (spike S4)

**Status:** proposed; the measurements on real hardware are pending.

## Context

`docs/PLAN.md` opens an item's web page in a tab of the main window,
rendered by Servo with JavaScript, created when the first tab opens and
dropped when the last one closes. S4 tries both ways of getting Servo's
frames into Slint (sharing GPU textures, reading frames back to the CPU),
checks that Blitz and Servo can share one `stylo`, and records memory with
one and three tabs and after closing them all.
*Decides:* frame path and Slint renderer, in-process vs. helper process.

## What was tried

### Servo version and dependencies

- **Servo 0.5.0** from crates.io (MPL-2.0), behind the `servo` feature (off
  by default for now). 0.6.0 (September 2026) is newer, but it moved to
  stylo 0.21, while Blitz 0.3.0-beta.2 uses stylo 0.20, as does Servo 0.5.
  With 0.5 the two share one stylo (`cargo tree -i stylo` resolves to a
  single package). **Unification therefore ties the Servo version to the
  Blitz version:** each upgrade has to wait until both are on the same
  stylo.
- Servo adds 434 crates, among them SpiderMonkey (`mozjs_sys`, whose build
  downloads a prebuilt library from GitHub), WebRender, surfman (OpenGL
  through EGL), a tokio runtime in its networking, and aws-lc-rs next to
  our ring. A debug build of the whole workspace with Servo needs about
  18 GB of disk (mostly debug info); CI builds it without debug info in a
  job of its own.
- **Licence:** servo-net enables async-tungstenite's `webpki-roots`
  unconditionally. Its data, Mozilla's CA list, is licensed
  CDLA-Permissive-2.0, which is not on the plan's list. It is a permissive
  data licence without conditions on redistributing results, but per the
  plan "anything unclear" needs a decision. `deny.toml` has an exception for
  that one crate, **pending approval**. Alternatives: patch servo-net to use
  the platform verifier, or wait for upstream to make it optional.
- The embedder has to pick rustls' crypto provider, as Servo's networking
  has both ring and aws-lc-rs; ren installs ring's, which it already uses.

### The engine (`tabs/`)

The UI only sees an `Engine` trait: open/close/activate tabs, resize, input,
dark mode, and per wake-up a list of events (title, loading, cursor,
failure) and possibly a new frame. `Browser` wraps Servo and one `WebView`
per tab: inactive tabs are hidden and throttled, only the active one is
painted, and Servo's wake-ups (from any thread) queue one `page-pump` on
Slint's event loop.

`HeadlessContext` is an offscreen OpenGL context on the **hardware** adapter
(surfman, EGL on Wayland or X11). Servo's own `SoftwareRenderingContext`
uses the software adapter (slow), and its `WindowRenderingContext` would
need the Slint window's surface, which the software renderer presents to
itself.

### Frame paths

- **CPU readback** (`--tabs helper`, `--tabs in-process`): after Servo
  painted, `glReadPixels` reads the frame into a Slint `SharedPixelBuffer`
  (two alternate, like the Blitz view), flipped in place, and shown as an
  image. Works with any Slint renderer, including the default software
  renderer, and from another process.
- **wgpu texture** (`--tabs wgpu`, feature `servo-wgpu`): Slint renders with
  femtovg-wgpu on wgpu's Vulkan backend. Each frame is blitted on the GPU
  from Servo's framebuffer into a Vulkan image whose memory OpenGL imports
  (`GL_EXT_memory_object_fd`); Slint draws that image as a wgpu texture.
  Follows Slint's `examples/servo`, but keeps two shared images instead of
  allocating one per frame. There are no semaphores between OpenGL and
  Vulkan, so `glFinish` waits for the blit. The texture must live on Slint's
  device, so this path needs Servo **in the UI process** (sharing across
  processes would need the image's file descriptor passed over a socket and
  imported on both sides; not tried).

### In-process or helper process

- **Servo can't be started twice in one process.** `Servo::new` stores its
  options in a `OnceLock` (`servo-config` `opts.rs`), which panics with
  "Already initialized" when set again; confirmed by trying it. So in the UI
  process, "drop Servo when the last tab closes" means no more tabs until
  ren restarts, or Servo has to stay alive once started. `--tabs in-process`
  drops it and then refuses to open tabs.
- **Helper process** (`--tabs helper`, the default): the same binary with
  `--servo-helper <socket>`, started with the first tab and ended with the
  last. Commands go as JSON lines over its stdin (closing stdin ends it);
  events and frames come back over a Unix socket (stdout could get output
  from Servo or its libraries). A frame is width, height, readback time and
  the RGBA pixels: one more copy through the kernel than in-process.
  Starting the process takes about 20 ms until it accepts tabs.
- **Failures stay in the helper.** In the container without libEGL, surfman
  panicked while creating the context; the helper died, the UI reported "the
  Servo helper exited", and ren went on. In-process, the same panic ends
  ren. Servo's content crashes would be contained the same way.

### Spot checks in the container

Under Xvfb with Mesa's llvmpipe (OpenGL) and lavapipe (Vulkan), debug
builds, three local test pages with a script: all three variants render the
pages (with JavaScript), switch and close tabs, scroll, and the helper can
be started again after closing all tabs. Not representative for memory or
speed (debug build, software GL); only the shape:

- Helper: the UI process grows by 7–8 MiB RSS with tabs open; after
  closing them its anonymous memory is back where it was (16.0 MiB), only
  file-backed pages of the code it ran stay (+3.6 MiB RSS). Servo's memory
  is in the helper and is gone when it exits.
- In-process: about 70 MiB of the UI process's anonymous memory stays after
  Servo was dropped (16 → 87 MiB anon).

## Measurements

To be run on real hardware (release build, real pages from the dump):

```
just measure-tabs DUMP_DIR [URL...]
```

| variant | before tabs | 1 tab | 3 tabs | after closing all | 1 tab again | after closing again | scrolling frames in 3 s | frame to UI ms |
|---|---|---|---|---|---|---|---|---|
| helper process, readback | | | | | | | | |
| in-process, readback | | | | | | | | |
| in-process, wgpu texture | | | | | | | | |

Scrolling smoothness (subjective): pending.

## Decision (proposed, pending the measurements)

- **Servo runs in a helper process**, started with the first tab and ended
  with the last: it is the only way to both free Servo's memory and open
  tabs again, and it keeps Servo's crashes out of the UI.
- **Frames are read back to the CPU**, so the default software renderer
  stays and nothing GPU-related is loaded until a tab opens. The wgpu path
  would make femtovg-wgpu (a GPU stack, loaded at startup) the renderer and
  require Servo in the UI process; it only wins if readback scrolling is
  noticeably worse on real hardware.
- **Servo 0.5.0**, pinned with Blitz's stylo, pending the licence decision.

## Open points

- Approve or reject the `webpki-roots` (CDLA-Permissive-2.0) exception.
- Readback cost per frame at full window size, and whether scrolling feels
  smooth enough (the helper adds a copy through the socket; shared memory
  would avoid it).
- Not done in the spike: popups (`target=_blank`, `window.open`) are
  ignored, no navigation buttons, no IME, context menu or file dialogs, keys
  with modifiers also go to the window's shortcuts.
