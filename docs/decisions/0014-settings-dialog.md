<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0014 — Settings dialog and account setup (M8b)

**Status:** done (2026-10-12), for review; checked in the container
under Xvfb against a mock Nextcloud, not yet on the desktop (see "On the
desktop"). Changed after three reviews (see "After the review");
measured on the desktop.

## Context

Until now the settings file was edited by hand (M4, `0008`), read again
when the window was activated (which never happened while the search
field had the focus, `0009`), and the app password came from `ren
--set-password`, `password-command` or `REN_APP_PASSWORD`. The plan's
M8b asks for a Zed-style settings dialog generated from the setting
descriptors, settings for the pane arrangement, the item colours and
the image cache, an account page with Nextcloud's Login Flow v2, a
manual app password and logging out, the first run opening that page,
edits by hand applying when the file is saved (inotify), and Help →
About ren with the third-party notices.

## What was built

### The settings dialog

Settings → Configure ren… opens a **window of its own**
(`SettingsWindow`), made when it opens and dropped when it closes:

- a search field and the categories on the left (Account, Sync, Article
  list, Articles, Web pages, Window), "Open Settings File" below them;
- per setting a row generated from its descriptor: title and
  description, and the control of its kind on the right, flush with its
  edge: a switch, a drop-down (choice), pictures for the arrangement, a
  number field with its unit inside, after the number (`NumberField`), a
  text field, or a colour picker; a list (one item per line) is edited below the
  description, over the row's width; "Reset to Default" when the file
  sets it;
- while searching, the rows of all categories that match (every word in
  the title, description, key, category or an option's title), with the
  category as a heading; the category list shows the number of results
  per category, and choosing one narrows the results to it;
- a setting that requires a switch (`Setting::requires`: closing to the
  tray needs the tray icon, the filter lists and the sites without
  blocking need blocking) is greyed out and disabled while the switch is
  off, and says which one it needs.

**Live apply:** every change is written at once (one value, through
`toml_edit`, M4) and applied: a switch or drop-down when it changes, a
text field on Enter or when the focus leaves it, a list when the focus
leaves it. A value that isn't valid stays in its field with the error
under the row, and the file isn't touched. A file with errors shows them
in a banner, with `file:line:column`, and the controls are disabled
until it's fixed: `set` refuses such a file (M4). "Open Settings File"
opens the file with `xdg-open`, made first with a comment if missing.

The rows come from **`settings_dialog.rs`**, a view model without Slint
(`Dialog`: categories, search, rows with their controls, what an edit
does; tested). Rows are updated in place when the same settings stay
shown, so a field keeps the focus; Slint drops a property's binding once
the user changed the control, so the controls take the row's values
again through `changed` handlers.

### New settings

In a new category, Article list:

- `list.arrangement` (`beside`, `above`): until now only the View
  menu's, for the run (`0013`). The View menu now changes the setting;
  `--arrangement` still forces one for the run without touching the
  file.
- `list.new-colour`, `list.unread-colour`: unset, the theme's colours
  for light and dark mode (Breeze's negative and positive text); set,
  one colour for both. A colour setting may now have no default
  (`Kind::Colour { default: None }`).

And `articles.image-cache-mib` (100, fixed until now, `0010`), 10 MiB to
100 GB. The image cache is shared by its clones (the limit an atomic),
so a new limit applies to the open article view; a smaller one removes
files when the next one is added.

### The settings file applied when it's saved

`ren_settings::watch` starts a thread (64 KiB stack) that sleeps on
inotify for the file's directory, and its target's if the file is a
symbolic link, and calls back when the file was written, replaced
(editors save by renaming a new file over the old one) or removed. The
window reads the file 100 ms later (editors may write in steps), and the
dialog follows. Through `libc`, in the build already; no new crate. This
replaces the reload on window activation; a sync still reads the file
when it starts.

### The account page

The Account category starts with a page made by hand:

- **Log In in the Browser** (Login Flow v2, `nextcloud_news::LoginFlow`):
  `POST /index.php/login/v2`, the login page opened in the browser
  (`xdg-open`), the poll endpoint asked every two seconds for 20 minutes
  at most. The page shows "waiting for the browser" with the address,
  "Open the Page Again" and "Cancel". 2FA and SSO work, as the browser
  handles them. Nextcloud lists the app password as a device named
  after the user agent (`ren/0.1.0`).
- **An app password created by hand**: login name and app password,
  checked with the server (`GET /version`) before they're kept.
- **Log Out**: revokes the app password on the server (`DELETE
  /ocs/v2.php/core/apppassword` with `OCS-APIRequest: true`; one that's
  revoked already counts as done), removes it from the Secret Service
  and unsets `account.user`. The server stays filled in, and so do the
  articles stored. It asks first, in a window of its own that the window
  manager decorates (`ConfirmWindow`, made on demand like About ren):
  "Revoke and Log Out" or Cancel (also Escape), as revoking can't be
  undone. A password from `REN_APP_PASSWORD` or the
  password command isn't revoked: ren didn't create it, and it may be
  in use elsewhere. Nextcloud revokes only the app password a request
  logged in with, and answers 403 ("no app password in use") to one that
  logged in with the account's own password (`AppPasswordController`);
  the page then says that nothing was revoked and suggests logging in
  through the browser.

A login writes `account.server` (the server's own address, if ren
accepts it) and `account.user`, stores the app password in the Secret
Service, keeps it for the syncs and starts one. The requests and the
Secret Service run on a thread of their own (`login.rs`), never on the
UI thread.

**Without a Secret Service** the password is kept until ren quits, and
the page says so. At the next start the first sync finds no password,
and the account page opens by itself with the reason and the fields
filled in: ren asks at every start, which `0008` left open. The page
also opens on the first run (no account; half a second after the start,
so the dialog comes in front of the main window: Slint can't make it a
transient window), when a sync is asked for without an account, and
after the first sync since the start if the server refuses the
password.

**Security:** the login page and the poll endpoint the server names must
be `https://` or on the server itself (`Error::Insecure` otherwise), so
neither the user's password nor the app password goes in plain text to
another host; the server address follows the rule of M4 (https, or http
on the local machine).

### About ren

Help → About ren is a small window of its own: the version, the GPL
notice, a link to the source, credits, Slint's "Made with Slint" badge
(`AboutSlint`), and "Third-Party Licenses", which turns the window into
a page of `THIRD-PARTY-NOTICES.html` (embedded in the binary), with
Back to return. `notices.rs` turns the HTML into headings, paragraphs
and licence texts, which keep their lines and are set in monospace; a
`ListView` lays out only the blocks in view, and they exist only while
the page is shown. Slint draws them, not Blitz: the page is the same
with and without `html-view`, and costs no second HTML view. M11
generates the file with cargo-about; until then it is a page written by
hand with the licences of the bundled Lucide icons and a note on the
crates, to be replaced.

## Choices

- **A window of its own** for the dialog, as in Zed, rather than a page
  in the main window: it costs memory only while open, and the main
  window keeps showing the effect of a change (the arrangement, the
  colours).
- **Server and user aren't rows:** the login sets them; they are still
  found by the search, and the file can be edited by hand.
- **The colour picker is ren's own** (`setting_controls.slint`): Slint
  has none. A square of saturation and value and a bar of hues drawn
  with gradients, Breeze's colours to start from, and the colour as
  `#rrggbb` to type, which is what the file holds.
- **Logging out keeps the articles.** Logging in as another account
  then makes the sync refuse the database (M5), as before; see "Open".
- **The settings dialog in demo mode** (`--items`, `--dump`) says that
  demo data has no settings, as "Allow Ads Here" does.

## After the review

The first review asked for five changes, all made:

1. **Settings that depend on each other:** descriptors name the switch
   they require; the dialog disables and greys out the setting while it
   is off (and `Settings::effective` says the same).
2. **Search and the category list:** the list shows the results per
   category while searching (none: greyed out); a category narrows the
   results without ending the search, choosing it again shows all, and
   clearing the search shows the category chosen last.
3. **Log Out** is on the right, beside who is logged in, like the other
   rows' controls.
4. **Colour pickers** instead of text fields: a button with the colour
   in effect (the theme's while unset) opens the picker; Apply sets it,
   "Theme's Colour" resets it.
5. **The arrangement as pictures:** a small drawing of the window per
   arrangement, the one in effect outlined in the accent colour.

The second review asked for four more:

1. **One alignment for the controls:** everything in the control column
   ends flush with its right edge; numbers are short fields with the
   number right-aligned and the unit after it (the unit had a fixed,
   left-aligned box), drop-downs and colour buttons have a fixed width,
   text fields the column's.
2. **The arrangement's description** doesn't name akregator.
3. **Lists below the description,** over the row's width; their control
   column only holds the reset button.
4. **The licenses inside About ren** instead of the article view (above).

The third review asked for two more:

1. **The unit inside the number field,** after the number, as
   akregator's and Zed's settings show it (like a spin box's suffix), so
   the number fields line up whatever their unit. Slint's `LineEdit` has
   no suffix, so `NumberField` draws a field like it from the palette.
2. **No keys in the dialog:** the rows showed each setting's key in the
   file under its description; Zed's dialog doesn't, and for most users
   it is noise. The search still finds a setting by its key.

## Testing

- `nextcloud-news`: the login flow against the mock server (the
  requests, form-encoded token, polling until done), refusing an
  endpoint in plain text elsewhere, a server that isn't a Nextcloud,
  revoking (also a password revoked already).
- `ren-settings`: watching (writes, renames, removal, other files,
  stopping, the target of a link, parsing events), the new settings,
  search, input parsing (numbers, colours with and without `#`, lists,
  empty text unsets) and server addresses typed without a scheme.
- `ren`: the dialog's rows, controls, search and edits
  (`settings_dialog.rs`); logging in through the browser and with an
  app password, without a Secret Service, cancelling, failures, logging
  out (`login.rs`, against the mock server with the in-memory store);
  the messages of the account page; the image cache's limit changed
  through a clone; `Reader::clear_current`.
- By hand in the container under Xvfb with Openbox, against a mock
  Nextcloud (Login Flow v2 and the News API serving the handwritten
  fixtures): the first run opening the account page, logging in through
  the browser (the waiting state, settings written, the first sync with
  the password kept in memory), logging out (the revoke request), a
  wrong and a right app password, the restart asking again without a
  Secret Service; the dialog's categories, search, a number refused and
  accepted, a switch, Reset to Default, an edit by hand applied while
  the dialog is open, a file with errors (banner, disabled controls),
  the list editor; the arrangement from the dialog and the View menu
  (the file follows, back to the default removes the key); About ren and
  the notices (plain text).

After the review, by hand in the same setup: closing to the tray
disabled with the tray icon off and enabled again at once; searching
"icon" (counts 1 and 2), narrowing to Window and back, clearing; a
colour picked from the square and a preset, one typed as hex, the list's
colours changing in the main window; the arrangement chosen by its
picture; both in light and dark mode. `settings_dialog.rs` tests the
disabled rows, the counts and narrowing, and picked colours.

## Measurements

`just measure-settings` (`ren --measure-settings` in a home of its own),
in the container under Xvfb, release build without the rendering
engines (`FEATURES=renderer-software,servo`), three runs; RSS in MiB:

| run | at the start | settings open | web pages shown | settings closed | About open | About closed |
|---|---|---|---|---|---|---|
| 1 | 29.8 | 33.2 | 33.3 | 30.9 | 32.0 | 30.9 |
| 2 | 29.8 | 33.2 | 33.2 | 30.8 | 31.9 | 30.8 |
| 3 | 29.8 | 33.0 | 33.2 | 30.8 | 31.8 | 30.8 |

After the second review, with the licenses page as a step of its own
(the same setup):

| run | at the start | settings open | web pages shown | settings closed | About open | licenses shown | About closed |
|---|---|---|---|---|---|---|---|
| 1 | 29.2 | 32.7 | 33.0 | 30.6 | 31.7 | 31.8 | 30.7 |
| 2 | 30.0 | 33.5 | 33.8 | 31.3 | 32.4 | 32.6 | 31.5 |
| 3 | 30.1 | 33.6 | 33.9 | 31.4 | 32.5 | 32.7 | 31.6 |

The dialog costs 3.2–3.5 MiB while open (its window's buffer, the
rows); 1.0–1.4 MiB stays after it closed (not broken down: probably
glyphs and Slint's caches of the second window's fonts). About ren
costs about 1 MiB and gives it back; the licenses page 0.1–0.2 MiB with
the short placeholder file (the generated one of M11 will be longer).
The watcher thread costs nothing measurable.

On the desktop (KDE on X11, release build with the default features, at
e4e534f, before the second review), by the user:

| run | at the start | settings open | web pages shown | settings closed | About open | About closed |
|---|---|---|---|---|---|---|
| 1 | 46.6 | 56.9 | 57.3 | 47.9 | 52.1 | 48.0 |
| 2 | 47.4 | 57.4 | 57.7 | 48.3 | 52.5 | 48.4 |

There the dialog costs 10.3–10.7 MiB while open, three times the
container's, and About ren 4.1–4.5 MiB; 0.9–1.3 MiB stays after the
dialog closed, as in the container, and About gives back all but
0.1 MiB. The difference is likely the desktop's window size, scale
factor and fonts (a larger window buffer for the software renderer, more
glyphs); not broken down.

## On the desktop

Not done yet; to check on the desktop (KDE):

- `just measure-settings` again, for the licenses page (the table above
  is from before it).
- Log in through the browser with the real server: 2FA if enabled, the
  app password in KWallet (`just check-secret-service` says whether it's
  stored), the device "ren/0.1.0" in Nextcloud's security settings;
  restart (no question); log out (the device gone).
- Edit `settings.toml` in an editor while ren runs: the change applies
  on save, an error shows in the status bar and the dialog.
- Help → About ren → Third-Party Licenses, and Back.

## Open

- **Another account than the database's:** logging in as someone else
  makes the sync refuse the database ("move it away to start anew");
  the account page could offer to start anew.
- **Dialogs aren't transient windows** of the main window (Slint can't
  set that): the window manager places them on their own, and the first
  run waits half a second so the dialog comes in front.
- **Wrapped text and window sizes:** Slint counts wrapped text as one
  line in a window's preferred height, so About ren has a fixed height.
- **Unknown keys** of the file (typos, a newer ren) aren't shown in the
  dialog, only as warnings in the status bar.
- **Third-party notices** are a placeholder until M11.
- Escape doesn't close the dialogs.
- The theme's colours on the colour buttons are read when the dialog
  refreshes, not when the desktop switches between light and dark mode
  while it is open.
