<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0012 — Content blocking in the tabs (M7b)

**Status:** in progress (2026-10-11): research done, the design below is
being built; the measurements on the desktop are still to come.

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

## Design

### The helper

- **The engine** is loaded on a thread when the helper starts (or when
  ren sends new lists): from its saved form in the cache directory if
  that was made from the same list files (paths, sizes, modification
  times) by the same adblock version, else from the lists, which it then
  saves. A load that comes before the engine is ready waits for it (at
  most the load's 50 ms, or a build's tenth of a second); if loading
  fails, nothing is blocked.
- **The check** (`load_web_resource`): the main frame's document is
  never blocked (the reader asked for that page), nor anything in a tab
  whose site is an exception; everything else is checked with the tab's
  URL as the page. Blocked requests are cancelled (a network error for
  the page). `$redirect` rules block without the replacement scripts
  (uBlock's resources aren't loaded); `$removeparam` and `$csp` are not
  applied.
- The fetch destination maps to adblock's request types: `script` and
  workers to script, `style` to stylesheet, `iframe`/`frame` to
  subdocument, `audio`/`video`/`track` to media, `object`/`embed` to
  object, an empty one (`fetch()`, XHR, beacons) to xmlhttprequest, and
  the rest to other.

### The window

- **Settings:** `pages.block-content` (on by default),
  `pages.filter-lists` (the default set above, by name, or URLs of other
  lists) and `pages.unblocked-sites` (host names; a site covers its
  subdomains). ren-settings gets a list kind for the last two (TOML
  arrays of strings).
- **Downloads** as above, then the list files go to the helper with
  `Command::Start` and, when they or the settings change,
  `Command::Blocking`.

### Popups

A page may open a tab only right after a click or a key press in it:
the helper notes the time of the last mouse button or key pressed in
each tab (input only goes to the active tab), and `request_create_new`
allows a tab within 5 s of it (browsers' transient activation, Servo's
`dom_transient_activation_duration_ms`), once per gesture (the
activation is consumed, as browsers do for popups). Pages may have at
most 10 tabs open that they opened themselves, of the 50 in all.
