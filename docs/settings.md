<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Settings

Ren's settings are in `~/.config/ren/settings.toml` (or under
`$XDG_CONFIG_HOME`). Settings → Configure Ren… changes them, or you edit
the file by hand: Ren applies it when you save it, and keeps your
comments and formatting when it changes a value itself. The file only
holds what differs from the defaults; resetting a setting removes it. If
the file has an error, Ren keeps the settings it had and shows the error
with its line and column.

```toml
[account]
server = "https://cloud.example.org"
user = "alex"

[sync]
interval-minutes = 30

[pages]
unblocked-sites = ["example.org"]
```

## Account

| Key | Default | |
|-----|---------|---|
| `account.server` | — | The address of your Nextcloud, e.g. `https://cloud.example.org`. Only `https://`, or `http://` on the machine itself (`localhost`, `127.0.0.1`, `[::1]`). |
| `account.user` | — | Your login name on the Nextcloud. |
| `account.password-command` | — | A command that prints the app password, e.g. `pass show nextcloud/ren`, for systems without a Secret Service (GNOME Keyring, KWallet). Used instead of the Secret Service when set. |

Logging in (Settings → Configure Ren… → Account) sets the server and the
user and keeps the app password in the Secret Service. Without a Secret
Service and without `password-command`, the password lasts until Ren
quits.

## Sync

| Key | Default | |
|-----|---------|---|
| `sync.interval-minutes` | `15` | Minutes between automatic syncs, up to a day. `0` syncs only when asked to. |
| `sync.keep-read-days` | `30` | Days to keep read items that aren't starred, counted from when they were read. `0` keeps them all. |
| `sync.feed-icons` | `true` | Download the feeds' icons from their sites after a sync, again when they are a month old. When off, no requests go to the sites; icons downloaded before are still shown. |

## Article list

| Key | Default | |
|-----|---------|---|
| `list.arrangement` | `"beside"` | Where the list of articles goes: `"beside"` the article, with two lines per article, or `"above"` it, as a table. Also in the View menu. |
| `list.new-colour` | a red | The colour of articles that arrived with the latest sync, as `"#rrggbb"`. Unset, it suits the light or dark colour scheme. |
| `list.unread-colour` | a green | The colour of unread articles, as `"#rrggbb"`. Unset, it suits the light or dark colour scheme. |

## Articles

| Key | Default | |
|-----|---------|---|
| `articles.load-images` | `true` | Load the images of articles from their servers. When off, they are loaded only on request (Article → Load Images), so the servers don't learn which articles you read; images loaded before are still shown. |
| `articles.image-cache-mib` | `100` | Disk space for the images of articles, in MiB (10 to 100,000), so that articles read again show them without fetching them, also offline. The images used least recently are removed first. |

## Web pages

| Key | Default | |
|-----|---------|---|
| `pages.keep-site-data` | `true` | Keep the cookies and site data of pages opened in tabs, so logins and consent choices last. When off, they are forgotten when the last tab closes. Settings → Clear Web Site Data removes what was kept. |
| `pages.block-content` | `true` | Don't load what the filter lists name as ads or trackers. The lists are downloaded when a page is opened and they are older than four days (`ren --update-filters` downloads them at once). |
| `pages.filter-lists` | `["easylist", "easyprivacy", "ublock", "ublock-privacy", "ublock-unbreak"]` | The lists of ads and trackers, by name (also `easylist-germany`, `fanboy-annoyance`), or the `https://` URL of another list in Adblock Plus or uBlock Origin syntax. |
| `pages.unblocked-sites` | `[]` | Sites on which nothing is blocked, e.g. because they don't work with blocking: host names such as `example.org`, each with its subdomains. "Allow Ads Here" in a page's navigation bar adds its site. |

Per feed, "Show Full Pages Instead of Articles" in the feed's context
menu opens the pages of its articles in a tab instead of the articles,
for feeds that only send a teaser.

## Window

| Key | Default | |
|-----|---------|---|
| `window.tray-icon` | `true` | Show an icon with the number of unread articles in the desktop's system tray (Plasma; GNOME needs the AppIndicator extension), with a menu to show or hide the window, fetch the feeds and quit. |
| `window.tray-icon-style` | `"colour"` | The logo `"colour"`ed, or in one colour like the desktop's symbolic tray icons: `"light"` for a dark panel, `"dark"` for a light one, or `"monochrome"`, following the colour scheme. |
| `window.close-to-tray` | `false` | Closing the window hides it in the tray instead of quitting, so the feeds are still fetched; the tray's menu quits. Only with the tray icon, on a desktop that shows it. |

The window's size and position, the panes, columns and sorting aren't
settings: Ren remembers them in `~/.local/state/ren/state.toml`.
