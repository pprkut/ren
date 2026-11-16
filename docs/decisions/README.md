<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Decision records

What was decided while building Ren, why, and what was measured on the
way. The records are history: they describe the state at their date, and
a later record supersedes parts of an earlier one rather than changing
it. How Ren is built now is in [`../ARCHITECTURE.md`](../ARCHITECTURE.md).

Ren was built after a plan of spikes (S1–S4), then milestones (M1–M11).
The records refer to it as "the plan"; it was removed once it was done
and is in the history: `git log --diff-filter=D -- docs/PLAN.md` finds
the commit that removed it, and `git show <commit>^:docs/PLAN.md` shows
it.

| | Record | What it decided |
|---|---|---|
| S1 | [0001 — UI toolkit](0001-ui-toolkit.md) | Slint, with the software renderer by default |
| S1b | [0002 — UI capabilities](0002-ui-capabilities.md) | Slint carries the desktop UI; own tree, table, splitter and tool bar; the Qt style as an opt-in feature |
| S2 | [0003 — Nextcloud News client](0003-nextcloud-client.md) | ureq and serde, 1000 items per page, streaming decode; the API's quirks |
| S3 | [0004 — Article view](0004-article-view.md) | Blitz 0.3 (pre-release) |
| S4 | [0005 — Web tabs](0005-web-tabs.md) | Servo in a helper process with CPU readback; the `webpki-roots` exception |
| M2 | [0006 — Store](0006-store.md) | SQLite through rusqlite; lists as ids in order |
| M3 | [0007 — Sync](0007-sync.md) | The sync engine, its cursor and the pending-change queue |
| M4 | [0008 — Settings](0008-settings.md) | `ren-settings`: the descriptor table, `toml_edit`, the Secret Service through `keyring-core` |
| M5 | [0009 — The app](0009-app.md) | The window on the store and a sync thread |
| M6 | [0010 — Articles](0010-articles.md) | The image cache, decoding at display size, glibc's mmap threshold |
| M7 | [0011 — Tabs](0011-tabs.md) | `ren-servo` as a program and workspace of its own; the frame protocol |
| M7b | [0012 — Content blocking](0012-content-blocking.md) | adblock-rust in the helper, network rules only |
| M8 | [0013 — Polish](0013-polish.md) | Window state, favicons, the tray (`ksni` exception), dialogs from pages |
| M8b | [0014 — Settings dialog](0014-settings-dialog.md) | The dialog from the descriptors, Login Flow v2, About Ren |
| M9 | [0015 — Tests](0015-tests.md) | Coverage, UI flows on Slint's testing backend, the end-to-end test |
| M10 | [0016 — Performance](0016-performance.md) | No renderer setting; frame pacing; the latest full measurements |
| M10b | [0017 — Release features](0017-release-features.md) | The name and logo, one instance, broken feeds, managing feeds, Servo's dialogs |
| M11 | [0018 — Release](0018-release.md) | Third-party notices, the application id, install layout, vendoring |
| — | [0019 — CI](0019-ci.md) | The workflows and checks; performance tracking with Bencher |

New records take the next number; see "Changes" in
[`CONTRIBUTING.md`](../../CONTRIBUTING.md).
