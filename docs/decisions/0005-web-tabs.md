<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0005 — Web pages in Servo tabs (spike S4)

**Status:** accepted (2026-10-04), including the licence exception for
`webpki-roots`. The Servo version and the helper being the same
binary as ren are superseded by `0011` (M7): Servo runs in a program of
its own and follows its releases.

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
  that one crate. Alternatives: patch servo-net to use the platform
  verifier, or wait for upstream to make it optional. *Approved in review:*
  the licence only asks that redistributors include its text, which the
  third-party notices (plan, M11) take care of; patching servo-net would be
  maintenance on every Servo upgrade.
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

`just measure-tabs DUMP_DIR` on an i7-1185G7 laptop (Iris Xe, Mesa), Linux
7.2, KDE on X11, release builds, the links of the first three items of the
real dump. Binary size: 31.1 MiB without Servo, 137.6 MiB with it,
142.9 MiB with `servo-wgpu`.

RSS in MiB; with the helper: total (UI process + helper).

| variant | before tabs | 1 tab | 3 tabs | after closing all | 1 tab again | after closing again | UI anon before / after closing | scrolling frames in 3 s | frame to UI ms (mean / max) | Servo start / stop ms |
|---|---|---|---|---|---|---|---|---|---|---|
| helper process, readback | 56.9 | 288.5 (76.4 + 212.2) | 418.0 (82.5 + 335.5) | 64.0 | 283.2 (76.5 + 206.7) | 64.0 | 10.5 / 16.8 | 127 | 6.87 / 20.83 | 6.1 / 30.2 |
| in-process, readback | 58.5 | 242.7 | 364.7 | 271.1 | - | - | 10.5 / 117.1 | 134 | 4.12 / 16.76 | 40.0 / 10.2 |
| in-process, wgpu texture | 116.9 | 251.5 | 372.0 | 287.5 | - | - | 34.0 / 133.1 | 129 | 5.41 / 23.72 | 39.1 / 9.0 |

- **Closing the tabs only gives the memory back with the helper:** 64.0 MiB
  afterwards, and the same again after a second round, so nothing
  accumulates. In-process, dropping Servo leaves 271–288 MiB (about
  107 MiB of it anonymous memory in the UI process), and Servo can't be
  started again there anyway.
- **The helper costs about 40–50 MiB more while tabs are open** (288.5 vs.
  242.7 MiB with one tab): a second process with its own heap and
  libraries, and in the UI process the two frame buffers and the frames
  coming in over the socket (+19.5 MiB with one tab). After closing, the
  UI process keeps 6.3 MiB more anonymous memory than before, freed frame
  buffers glibc doesn't return (as in S3); it doesn't grow with more rounds.
- **The wgpu renderer costs 60 MiB before any tab is open** (116.9 vs.
  56.9 MiB, 34 vs. 10.5 MiB anonymous) for the whole session, and the
  shared texture isn't even faster here: the GPU blit plus `glFinish` takes
  5.4 ms per frame, the readback 4.1 ms in-process and 6.9 ms with the
  transfer from the helper.
- **Scrolling:** 127–134 frames in 3 s in all three, and it feels fine in
  all three; the helper a bit smoother than the others (Servo's work doesn't
  run on the UI thread).
- **Starting and stopping:** the helper accepts tabs 6 ms after it is
  started (Servo itself then starts in the helper), and exits in 30 ms.

## Review (2026-10-04)

Found while trying it, fixed in this spike:

- After resizing the window the page kept its first size, and with the
  helper it stopped repainting until another tab was shown: the engine
  resized the GL context before the web views, and Servo only updates its
  viewport when it resizes the context itself. Now only the web view is
  resized.
- Selecting another item showed its article only in the hidden Article tab;
  the Article tab now comes to the front, and the page in the background is
  hidden and throttled.
- Links in the article did nothing. Now a click (any button, with or
  without modifiers) opens the link in a tab; the link's context menu has
  Open Link in Tab, Open Link in Browser (`xdg-open`) and Copy Link Address.
  Only `http`, `https` and `mailto` links are opened.

## Decision

- **Servo runs in a helper process**, started with the first tab and ended
  with the last. It is the only variant that gives the memory back and can
  open tabs again, it keeps Servo's crashes out of the UI, and it scrolls a
  bit more smoothly. It costs 40–50 MiB more while tabs are open.
- **Frames are read back to the CPU**, and **the software renderer stays the
  default** (0001): the wgpu path would cost 60 MiB for the whole session,
  needs Servo in the UI process, and wasn't faster.
- **Servo 0.5.0**, pinned so that it shares stylo with Blitz 0.3.0-beta.2;
  upgrades of the two have to be coordinated.
- The `servo` feature stays off by default until M7 makes the tabs
  production-ready; the wgpu path (`servo-wgpu`, `--tabs wgpu`) and
  `--tabs in-process` are spike code that M7 removes.

## Review of the code (2026-10-04)

- The UI accepted any frame size and event length from the helper and
  allocated accordingly; as the helper runs web content, that is now
  bounded (frames up to the largest size requested, events up to 1 MiB).
- For M7 (see the plan): keep Servo current instead of pinning it to
  Blitz's stylo, since the two run in different processes and sharing
  saves no memory at runtime; deny non-web navigation inside tabs; decide
  on a persistent Servo profile; an inherited socket pair instead of a
  socket path; don't wait for the helper on the UI thread.

## Scrolling heavy pages (2026-10-04)

Scrolling felt slow on script-heavy pages (heise, Ars Technica) after
accepting their consent banners. Measured with `REN_TAB_STATS=1` and
per-thread CPU sampling while scrolling a heise article and a plain text
page with 60 mouse wheel steps in 3 s (release build, i7-1185G7, Iris Xe):

| per frame (mean / max) | plain page | heise after consent |
|---|---|---|
| frames per second | 29 (one per wheel step) | 29–68; still ~24 after scrolling stopped |
| Servo `paint` | 0.3 ms | 0.5–2.3 ms |
| readback (`glReadPixels`) | 3 ms | 4–22 ms, up to 160 ms |
| send to the UI | 2.7 ms | 2.5–10 ms |
| helper CPU | 22 % of a core | 90 % (page script 42–46 %, styling ~13 %) |
| UI CPU (software renderer) | 29 % | 55 % (drawing 42 %, receiving 13 %) |

- Servo's painting is cheap, and its scrolling doesn't wait for the
  page's scripts. **The frame path is the bottleneck:** `glReadPixels`
  waits for the GPU to finish the frame, then the frame is copied through
  the socket and drawn again by the UI.
- **Heavy pages repaint all the time** (ads, animations): about 24 frames
  per second without any input, each paying the whole frame path, so
  scroll frames queue behind them.
- Servo has **no smooth wheel scrolling**: every wheel step is one jump.

The UI renderer doesn't change this. Same test on heise:

| UI renderer | UI CPU (drawing) | frames/s | readback mean / max |
|---|---|---|---|
| software | 55 % (42 %) | 28–68 | 4–18 / 160 ms |
| FemtoVG | 26 % (14.5 %) | 29–90 | 5–12 / 36 ms |
| Skia | 36 % (22 %) | 30–73 | 6–21 / 48 ms |
| wgpu textures (Servo in-process) | 93 % incl. Servo, 19 % drawing | 37–65 | none |

GPU renderers halve the UI's drawing cost, but the readback in the helper,
the constant repaints and the missing smooth scrolling stay, and in use
all variants felt the same. The software renderer stays; the frame path is
fixed in M7 and the renderers compared again in M10.

Also found: when the UI process is killed (not closed), the helper isn't
told to quit by ren, only by its stdin closing, and Servo's shutdown can
then hang: one orphaned helper was still running after 10 minutes, another
exited by itself. M7 makes the helper die with its parent and bound its
own shutdown time.

## Open points

- M7: frames through shared memory instead of the socket (one copy less),
  and giving the UI process's frame buffers back after the last tab closes
  (`malloc_trim` or buffers outside the heap).
- Not done in the spike: popups (`target=_blank`, `window.open`) are
  ignored, no navigation buttons, no IME, context menu or file dialogs, keys
  with modifiers also go to the window's shortcuts.
