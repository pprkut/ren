<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0012 — Content blocking in the tabs (M7b)

**Status:** done (2026-10-11), for review; measured on the desktop on
2026-10-11.

## Context

M7 (`0011`) left heavy pages heavy: a heise tab costs the helper 470
MiB once its consent banner is accepted (400 before), 1.15 GiB with
three; Ars Technica about 160 MiB per tab. Most of it is ads and
trackers, which also repaint all the time and send the reader's
browsing to third parties. The plan's M7b asks for research first
(engines, lists, cosmetic filtering, cost), then blocking in the helper,
a setting to turn it off (on by default), and per-site exceptions. It
also took over one point of M7: pages open tabs without a user gesture,
up to the limit of 50.

## Research

### Servo's hook

Servo 0.7 has one place to block requests:
`WebViewDelegate::load_web_resource(webview, WebResourceLoad)`, called
for every HTTP(S) fetch of a web view (loads without one, e.g. of
service workers, go to `ServoDelegate::load_web_resource`). The
embedder lets the load go on by dropping it, or calls
`intercept(response)` and then `cancel()` (a network error for the
page) or sends a body of its own. What Servo tells about the request
(`WebResourceRequest`): method, headers, URL, the fetch destination
(`script`, `style`, `image`, `iframe`, `document`, … or empty for
`fetch()`/XHR), the referrer, whether it's the main frame's document,
and whether it's a redirect.

How it works inside (`servo-net`, `request_interceptor.rs`):

- **Every fetch already goes through the helper's main thread,** blocking
  or not: the net thread sends the request to the embedder and waits for
  the answer, which the helper gives in `spin_event_loop` (the default
  delegate method drops the load at once). So the check costs only the
  matching itself, not a new round trip.
- **One request at a time:** the interceptor sits behind a mutex that is
  held until the embedder answered, so requests are checked one after
  another. Matching must be fast and must not wait for anything; a
  request held back (e.g. until the filter engine is loaded) holds up
  all the others.
- **No frame or page URL:** only the referrer, which pages can strip
  (`Referrer-Policy: no-referrer`) or shorten to the origin. Filter rules
  need the page a request is made for (`$third-party`, `$domain=`), so
  the helper uses the tab's current URL, which it knows from
  `notify_url_changed`. For requests of iframes that is the top page,
  not the frame: rules that name the frame's site don't apply there
  (uBlock Origin uses the frame's).
- **WebSockets** don't go through the hook.

Servo 0.7 itself has no content blocking, only this hook. Popups: Servo 0.7 tracks user activation
(for the Fullscreen API), but `window.open` doesn't check it, so every
`window.open` reaches the embedder's `request_create_new`.

### Engines

- **Brave's `adblock` crate** (adblock-rust, 0.13.3, MPL-2.0): the engine
  of Brave's blocker. It parses uBlock Origin and Adblock Plus syntax
  (network rules with their options, exceptions, `$important`,
  `$redirect`, `$removeparam`, cosmetic rules and scriptlets) into a
  flatbuffer that can be saved and loaded again. 6 new crates in the
  helper's build (`adblock`, `addr`, `flatbuffers`, `psl`, `psl-types`,
  `seahash`; MPL-2.0, MIT/Apache-2.0, Apache-2.0, MIT), the rest is
  already in Servo's. Its `single-thread` feature (on by default) makes
  the engine `!Send`; without it the engine can be built on a thread and
  handed to the main thread, at the same speed and memory (measured
  below).
- **A host list** (a set of host names, as in hosts files or Peter
  Lowe's list): simpler, but it can't express paths, request types,
  first/third party or exceptions. From EasyList and EasyPrivacy's
  `||host^` rules alone (95,888 hosts) it blocked 36.6 % of the sample
  requests below, against 42.6 % for the engine with its exceptions;
  without the exceptions it also blocks what they let through to keep
  pages working. With the engine's memory as low as measured, a host
  list saves nothing worth the lost precision.
- **Other Servo embedders** do it the same way: NeuNoah's `rust-browser`
  (Brave's engine, EasyList and EasyPrivacy on request) and kayfgit's
  `browser` (an engine it doesn't name; EasyList, EasyPrivacy, uBlock's,
  Peter Lowe's) both block in Servo's request hook; neither has cosmetic filtering for Servo or
  published measurements.

**Chosen: the `adblock` crate**, without `single-thread`, with its
built-in public suffix list (`embedded-domain-resolver`; Servo has one
too, `is_reg_domain`, but adblock wants the registrable domain, which
Servo doesn't export).

### Lists

| list | size | rules | licence | expires |
|---|---|---|---|---|
| EasyList | 2.1 MB | 81,387 lines | GPL-3.0-or-later or CC-BY-SA-3.0-or-later | 4 days |
| EasyPrivacy | 1.5 MB | 57,079 lines | as EasyList | 4 days |
| uBlock filters | 2.3 MB | 41,265 lines | GPL-3.0 | 5 days |
| uBlock filters – Privacy | 0.2 MB | 3,487 lines | GPL-3.0 | 7 days |
| uBlock filters – Unbreak | 0.3 MB | 6,007 lines | GPL-3.0 | 5 days |
| EasyList Germany | 0.3 MB | 6,056 lines | as EasyList | 1 day |
| Fanboy's Annoyance | 1.7 MB | 50,691 lines | as EasyList | 4 days |

(Sizes and lines of 2026-10-11, uBlock's with their includes. EasyList's
licence from its licence page, which the container couldn't reach;
uBlock's `LICENSE` is the GPLv3.)

- **uBlock's lists aren't complete files:** `filters.txt` pulls in ten
  others with `!#include` (`filters-general.txt`, one per year since
  2020, link shorteners; 0.5 MB of its own, 2.3 MB with them), and
  `privacy.txt` one; parts are only for some browsers or platforms
  (`!#if env_mobile`, `!#if env_firefox`, …, `!#else`, `!#endif`). The
  adblock crate handles neither: it reads the directives as comments, so
  the included rules would be missing and the conditional ones always
  apply. Brave prepares the lists before its engine loads them; ren does
  the same when it downloads them: includes from the same directory are
  inserted (one level, as uBlock does), and conditions are evaluated with
  only `ext_ublock` true (uBlock's syntax; no browser, not mobile).

- **The default set:** EasyList, EasyPrivacy, uBlock filters, uBlock
  Privacy and uBlock Unbreak, as uBlock Origin and Brave start with. The
  Unbreak list is exceptions for rules of the others that break pages
  (as measured with uBlock's top files alone, 343 of the sample's requests
fewer blocked).
- **Regional and annoyance lists** are options: EasyList Germany (for
  heise and the like), Fanboy's Annoyance (cookie banners, newsletter
  pop-ups, mostly cosmetic rules, so of little use without cosmetic
  filtering). The setting also takes other lists by URL.
- **Downloaded, not shipped.** The licences would allow shipping them
  (all GPL-3.0-compatible), but lists change several times a day: a
  copy in the repository or the package is out of date before it is
  installed, and 4 MB of third-party data would go into the repository
  and the notices. The cost: the first tab ever opened (and the first
  after a cache was cleared) loads without blocking until the lists are
  there, a second or two.
- **Updated by ren, not by the helper.** ren has an HTTP client with TLS
  already (ureq, as for the server and the images); the helper runs web
  content and lives only as long as tabs are open. When the helper
  starts, ren downloads each list that is missing or older than 4 days
  (the longest the lists ask for is 7, the shortest 1; 4 is EasyList's)
  on a thread of its own at `nice` 10, at most 16 MiB each, into
  `$XDG_CACHE_HOME/ren/filters/` (re-downloadable, so a cache; written
  to a temporary file and renamed, so the helper never reads half a
  list). A failed download keeps the old list. When lists changed, ren
  tells the helper, which loads them for the next requests. No periodic
  updates while tabs are open: the helper rarely runs for days.

### Cosmetic filtering

Hiding elements needs CSS, and for uBlock's scriptlets (`##+js`, 2,451 in
uBlock filters) JavaScript, injected into pages. Servo's
`UserContentManager` takes style sheets and scripts, but:

- they apply to every page of the web views that share the manager, not
  per site, and changes apply only to documents loaded afterwards;
- EasyList has 13,634 generic hiding rules (`##selector`, for every
  site) and 14,652 per site; one style sheet with all generic selectors
  is matched against every element of every page, which adblock-rust
  avoids in Brave with a script that watches the page's classes and ids
  and asks for the matching selectors (`hidden_class_id_selectors`);
- scriptlets are JavaScript run in the page before its own, which needs
  user scripts per site.

A manager per web view, updated with the site's selectors and scriptlets
when the main frame navigates (in `request_navigation`, before the
document loads) might work, but its timing with Servo's script threads
is untested, and the gain is mostly visual: what loads is what costs
memory and CPU, and network blocking already stops it. **Network
blocking only, for now;** cosmetic filtering goes to the backlog, to be
tried once network blocking is measured.

### Cost

In the container (Intel Xeon at 2.8 GHz, release build, glibc's mmap
threshold at 1 MiB as in the helper), with the lists of 2026-10-11 and
the first 2,000 and 100,000 requests of adblock-rust's sample
(`data/requests.json`, 242,944 requests recorded on real sites), network
rules only:

| lists | build from text | peak while building | engine saved | load saved engine | RSS after loading | + after 2,000 requests | blocked (100,000) | per request mean / p99 / max (100,000) |
|---|---|---|---|---|---|---|---|---|
| EasyList, EasyPrivacy | 0.12 s | +18 MiB | 4.6 MiB | 47 ms | +5.4 MiB | | 42.9 % | 6.1 / 32 / 1564 µs |
| + uBlock filters, Privacy, Unbreak (the default) | 0.16 s | +24 MiB | 5.3 MiB | 40 ms | +6.2 MiB | +4.9 MiB | 42.8 % | 9.8 / 46 / 1573 µs |
| default + EasyList Germany | 0.12 s | +24 MiB | 5.4 MiB | 34 ms | +6.5 MiB | +4.8 MiB | | |
| + Fanboy's Annoyance | 0.16 s | +26 MiB | 5.7 MiB | 42 ms | +6.5 MiB | | 43.5 % | 10.5 / 49 / 2192 µs |
| the same with cosmetic rules | 0.25 s | +40 MiB | 10.3 MiB | 55 ms | +11.2 MiB | | 43.5 % | 11.8 / 55 / 1636 µs |

uBlock's lists with their includes resolved and conditions evaluated as
described under "Lists" (a first run with uBlock's top files alone, a
fifth of its rules, used 0.5–2 MiB less).

- **About 11 MiB for the helper** with the default lists: 6.2 MiB for
  the loaded engine and 5 MiB that matching adds over the first couple
  of thousand requests (regular expressions compiled on first use), once
  per helper, not per tab. Against 160–470 MiB per heavy tab.
- **Building from the lists** takes a sixth of a second and 24 MiB for a
  moment; the helper saves the engine and loads it in about 40 ms next
  time, on a thread while Servo starts.
- **10 µs per request** on average, 46 µs for 99 %; the slowest, about
  1.5 ms, compile a regular expression. A page with 200 requests waits
  about 2 ms in total.
- Without `single-thread`: the same (with EasyList, EasyPrivacy and
  uBlock's top files: 8.1–9.5 µs per request, 5.4–5.5 MiB loaded, in two
  runs each).
- Cosmetic rules in the engine cost 4.7 MiB more even if unused, so the
  helper parses network rules only.

## What was built

### The helper (`ren-servo`)

- **The engine** (`blocking.rs`) is made on a thread when the helper
  starts, and again when ren sends new lists: from the network rules of
  the list files, or from its saved form (`engine.dat` next to them) if
  that was made from the same files (paths, sizes, modification times;
  a version of the engine that can't read it makes it again). Loads
  that come before it's ready wait for it, which holds up the next ones
  too (Servo checks one at a time), at most the load's 40 ms or a
  build's sixth of a second; new lists are loaded while the old engine
  goes on blocking. Without any lists, nothing is blocked.
- **The check** (`load_web_resource`): the main frame's document is
  never blocked (the reader asked for that page), nor anything when the
  tab's or the requesting document's site is an exception. The page a
  request is checked for is its referrer, else the tab's URL: Servo may
  tell the new URL only after the first requests of the next page, and
  the referrer is the document (or frame) that asked, as uBlock uses
  it. Blocked requests are cancelled (a network error for the page).
  `$redirect` rules block without the replacement scripts (uBlock's
  resources aren't loaded); `$removeparam` and `$csp` are not applied.
- The fetch destination maps to adblock's request types: `script` and
  workers to script, `style` to stylesheet, `iframe`/`frame` to
  subdocument, `audio`/`video`/`track` to media, `object`/`embed` to
  object, an empty one (`fetch()`, XHR, beacons) to xmlhttprequest, and
  the rest to other.
- **Popups** need a user gesture: the helper notes the last mouse button
  or key (not Escape or a modifier) pressed in each tab; a page may open
  one tab within 5 s of it (browsers' transient activation, Servo's
  `dom_transient_activation_duration_ms`), and the gesture is used up,
  as browsers do for popups. At most 10 tabs that pages opened may be
  open at a time, of the 50 in all. Refused requests make `window.open`
  return `null`.
- `REN_TAB_STATS` counts the requests checked and blocked;
  `REN_SERVO_NO_BLOCKING` turns blocking off, for `just measure-tabs`.
- New in the helper's build: `adblock` 0.13 without `single-thread`, and
  five small crates (above).

### The protocol (`ren-tabs`)

- `Command::Start` carries `Blocking` (the list files, where to save the
  engine, the sites without blocking), or none; `Command::Blocking`
  sends new ones.
- Which sites an exception covers (a host name covers itself and its
  subdomains), for both sides.

### The window

- **Settings** (ren-settings got lists of strings as a kind, TOML
  arrays):
  - `pages.block-content`, on by default;
  - `pages.filter-lists`: `easylist`, `easyprivacy`, `ublock`,
    `ublock-privacy`, `ublock-unbreak` by default; also
    `easylist-germany`, `fanboy-annoyance`, or the `https://` URL of
    another list;
  - `pages.unblocked-sites`: host names.
  Demo data (`--items`, `--dump`) has no settings file and blocks with
  the defaults.
- **Downloads** (`tabs/filters.rs`): when the helper starts (and when
  the lists in the settings change while it runs), the lists that are
  missing or older than four days are downloaded one after another on a
  thread at `nice` 10, with the HTTP agent of the article images (now
  `http.rs`), at most 16 MiB each with their includes, into
  `$XDG_CACHE_HOME/ren/filters/` (one file per URL, named after its
  hash; written to a temporary file and renamed). uBlock's `!#include`
  lines are replaced by the files they name, if those are next to the
  list (one level, at most 32), and `!#if` sections are evaluated with
  only `ext_ublock` true (`!`, `&&`, `||`, parentheses). A list whose
  include fails isn't written, and the old one stays. When something
  was written, the helper gets the lists again. One update runs at a
  time.
- `ren --update-filters` does the same from the command line and tells
  how it went, so measurements can start with the lists.
- **"Allow Ads Here" / "Block Ads Here"** in the page's navigation bar
  (while blocking is on): adds the page's site (without `www.`) to
  `pages.unblocked-sites`, or removes the sites that cover it; the file
  keeps its comments, the helper gets the change, and the page is
  reloaded.

## Checked in the container

Debug builds under Xvfb with Mesa's llvmpipe, a local test page with a
script and an image under `/ads/`, a third-party image (`127.0.0.1` on a
`localhost` page), a `fetch()` to `/track/beacon`, an allowed image and
`fetch()`, and a `window.open` when the page loads; a local list with
rules for them, and uBlock's filters by URL from GitHub (the container
can't reach easylist.to):

- The server saw only the page, the allowed image and the allowed
  `fetch()`; the helper blocked the script, the image, the third-party
  image and the beacon (the helper's statistics, which miss its last
  second: 6 of 9 requests checked blocked).
- uBlock's filters were downloaded when the helper started, with their
  ten includes (2.3 MB, as in the benchmark), the helper got them and
  saved its engine.
- Every `window.open` without a gesture was refused.
- With `localhost` in `pages.unblocked-sites`, everything loaded.
- `ren --update-filters` downloads what's due, says "up to date" for the
  rest, and fails (exit status 1) when a list can't be fetched, keeping
  the others.

## Measurements on the desktop

By the user, at 3d61438, on the machine of 0011 (i7-1185G7, Iris Xe,
Mesa, Linux 7.2.8, KDE on X11), release builds, with the profile of M7
(heise's consent accepted): `just measure-tabs ~/ren-dump` (the links of
the first three items), then the same with 0011's heise and Ars Technica
articles as the only page (all three tabs the same page). The lists were
downloaded first (5.9 MiB from easylist.to and uBlock's GitHub pages,
0.4 s). RSS in MiB, total (UI + helper):

| pages | variant | before tabs | 1 tab | 3 tabs | after closing all | 1 tab again | after closing again | UI anon before / after | scrolling frames in 3 s | CPU % scrolling (window / helper) | frame to UI ms (mean / max) | Servo start / stop ms | requests blocked / checked |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| dump | blocking | 48.1 | 253.1 (62.0 + 191.1) | 295.7 (62.1 + 233.6) | 49.0 | 255.5 (62.4 + 193.1) | 49.1 | 7.2 / 7.3 | 130 | 21 / 16 | 5.54 / 18.10 | 0.9 / 1.4 | 12 / 98 |
| dump | without | 48.5 | 246.9 (62.5 + 184.4) | 261.5 (62.6 + 198.9) | 49.5 | 246.8 (62.8 + 184.0) | 49.5 | 7.1 / 7.3 | 131 | 22 / 16 | 5.00 / 15.99 | 0.5 / 1.5 | 0 / 0 |
| heise | blocking | 47.9 | 505.2 (62.0 + 443.2) | 946.0 (62.1 + 884.0) | 49.0 | 548.6 (62.4 + 486.2) | 49.0 | 7.1 / 7.3 | 134 | 22 / 36 | 5.84 / 19.75 | 1.0 / 0.9 | 133 / 747 |
| heise | without | 48.9 | 565.4 (62.8 + 502.6) | 1063.1 (62.8 + 1000.3) | 49.7 | 705.0 (63.1 + 641.9) | 49.7 | 7.1 / 7.3 | 60 | 18 / 155 | 32.82 / 144.37 | 0.4 / 1.3 | 0 / 0 |
| Ars Technica | blocking | 48.0 | 324.7 (62.1 + 262.6) | 500.9 (62.1 + 438.8) | 49.1 | 326.6 (62.5 + 264.1) | 49.2 | 7.1 / 7.3 | 48 | 16 / 32 | 58.30 / 156.11 | 1.3 / 0.8 | 38 / 370 |
| Ars Technica | without | 48.4 | 421.0 (62.3 + 358.7) | 724.9 (62.4 + 662.5) | 49.3 | 424.7 (62.7 + 362.0) | 49.3 | 7.1 / 7.3 | 38 | 14 / 40 | 60.67 / 145.71 | 0.4 / 1.3 | 0 / 0 |

Binary sizes: ren 35.7 MiB (35.5 without the tabs; 35.5 and 35.4 in
0011, the downloads and settings), ren-servo 105.0 MiB (103.8: the
engine and the public suffix list).

- **Heavy pages get smaller:** the helper with one Ars Technica tab is
  96 MiB smaller (263 instead of 359, a quarter), with three 224 MiB
  (439 instead of 663, a third). heise: 59 MiB less with one tab (443
  instead of 503), 116 with three (884 instead of 1000), 156 with one
  tab again (486 instead of 642). The blocked requests are fewer than
  one might think (heise: 133 of 747, 18 %; Ars Technica: 38 of 370,
  10 %; the counts miss the helper's last second), but they are the
  ones that load more: ad scripts that fetch the ads, frames, trackers.
- **heise scrolls smoothly now:** 134 frames in 3 s instead of 60, the
  helper at 36 % of a core instead of 155 %, and 5.8 ms from a painted
  frame to the window instead of 33 (at most 20 ms instead of 144). The
  ads were what kept the helper busy; heise scrolls now like the light
  pages (130).
- **Light pages:** 6.7 MiB more in the helper with one tab and 9.1 with
  one tab again, as expected from the container (11 MiB with the regular
  expressions of 2,000 requests; these pages made fewer). With three
  tabs the run with blocking was 35 MiB larger, which isn't the
  engine's (it's there once); the dump's pages differ from run to run
  (0011 measured 245 MiB for the same step without blocking, this run
  199). 12 of 98 requests were blocked. Frames, CPU and the window are
  the same with and without.
- **Starting the helper** took 0.9–1.3 ms with blocking against 0.4–0.5
  without (sending the lists' paths; the engine loads on the helper's
  thread), stopping as before. The window is unchanged: 49 MiB after
  closing, its anonymous memory back at 7.3 MiB.
- **Ars Technica scrolls slowly in both** (48 and 38 frames in 3 s, 58
  and 61 ms from a painted frame to the window), against 162 frames and
  11 ms in 0011 with the same article. The light pages and heise are as
  fast as in 0011, so the frame path didn't change for the worse. The
  helper's statistics (`REN_TAB_STATS`) show where the time goes: while
  scrolling, Servo's paint takes 2–3 ms per frame (heise and the dump's
  pages: 0.3–0.9), but the readback waits 45–74 ms on average for the GPU
  to finish the frame, up to 147 ms (heise and the dump's pages: 4.7–6
  ms), 85–96 % of each second. The helper's loop polls the fence
  meanwhile and has nothing else to do. So this page is heavy for the
  GPU (WebRender on Mesa's Iris driver), not for Servo's CPU side or
  the frame path; content blocking doesn't change it, and two frames
  under way (M10) wouldn't either, as the GPU is the limit. The page
  has probably changed since 0011 (its media or effects); M10.

**By hand** (the user): heise and Ars Technica look the same with
blocking as without, as far as could be told.

## Open

- **Ars Technica's slow frames** (see above), with and without
  blocking: the GPU takes 45–74 ms per frame of this page. M10.
- **Cosmetic filtering** (element hiding, uBlock's scriptlets): not
  done, see "Cosmetic filtering"; in the plan's backlog. Consent banners
  stay.
- `$redirect` resources, `$removeparam` and `$csp` aren't applied;
  WebSockets and loads without a web view (service workers) aren't
  checked.
- Frames are checked as their referrer or the top page, so `$domain=`
  rules for a frame's own site may miss when the frame sends no
  referrer.
- The first tab ever (and after the cache was cleared) loads without
  blocking until the lists are downloaded.
- Lists aren't updated while the helper runs for more than four days.
- A refused popup isn't shown to the reader (browsers show a bar); a
  page that needs one shows nothing.
- EasyList's licence was taken from its licence page as remembered
  (dual GPL-3.0-or-later / CC-BY-SA-3.0-or-later); the container
  couldn't fetch the page to confirm. Only relevant if lists were ever
  shipped.
- The helper's last second of statistics before it ends isn't printed,
  so `just measure-tabs` may count a few requests fewer.
