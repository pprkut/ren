<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Ren

A native reader for the [News app](https://apps.nextcloud.com/apps/news)
of Nextcloud, written in Rust. Ren syncs the feeds, folders and articles
of your Nextcloud, and whether you read or starred them, and works from
its own copy, also offline. Articles are shown as the feed sends them;
the web page behind an article opens in a tab, rendered by Servo, with
ads and trackers blocked. Low memory and processor use come first.

蓮 (*ren*) is the lotus of its logo.

## Using it

Start `ren`; the first time, it asks you to log in to your Nextcloud in
the browser (an app password of its own, which your Nextcloud lists under
Security; two-factor authentication and single sign-on work), or to enter
an app password you made yourself. Settings → Configure Ren… has the rest,
which is kept in `~/.config/ren/settings.toml`, a file you may also edit
by hand. The app password is kept in the desktop's Secret Service (KWallet,
GNOME Keyring), or comes from a command (`password-command`, e.g.
`pass show nextcloud/ren`).

The welcome page lists the shortcuts for reading: J and K go to the next
and previous article, Space scrolls, M marks it read or unread, S stars
it, O opens its page in a tab.

## Building

Ren needs Rust 1.92 or newer and the fontconfig development files; the
web page tabs need what Servo needs to build (clang, Python 3, and for
SpiderMonkey a C++ compiler; see Servo's documentation). With
[just](https://just.systems):

```sh
just build                 # ren and its Servo helper, in target/release
sudo just install          # into /usr/local; just install /usr "$DESTDIR" for packages
```

The Servo helper (`ren-servo`) is a program of its own, in a workspace of
its own (`crates/ren-servo`): Servo and Ren's article view can't share a
build. It's installed into `libexec/ren/` and only started when a web page
opens. Without it, pages open in the browser; `cargo build --release`
alone builds Ren without it (NO_HELPER=1 just install).

`just dist` makes the release tarballs: the source, and the crates of
both workspaces for building without the network (`cargo build
--offline`, with `MOZJS_FROM_SOURCE=1` so SpiderMonkey isn't downloaded
either). `packaging/slackware/` has a SlackBuild.

## Developing

`docs/PLAN.md` is the plan, `docs/decisions/` has what was decided and
measured on the way, and `CLAUDE.md` the rules for working on it. `just`
lists the tasks: `just check-light` runs the checks without the rendering
engines, `just check` with them, `just test-nextcloud` the end-to-end
test against Nextcloud in a container.

## Licence

Ren is free software: you can redistribute it and/or modify it under the
terms of the GNU General Public License as published by the Free Software
Foundation, either version 3 of the License, or (at your option) any later
version (`LICENSES/GPL-3.0-or-later.txt`). Configuration files are under
CC0-1.0; the project follows [REUSE](https://reuse.software).
`THIRD-PARTY-NOTICES.html` lists the licences of what Ren is built from;
About Ren shows them too.
