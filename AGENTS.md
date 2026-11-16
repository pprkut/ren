<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Ren, for coding agents

Native Nextcloud News reader in Rust: Slint UI, Blitz article view, web
pages in tabs rendered by Servo in a helper program started on demand.

Read first: [`CONTRIBUTING.md`](CONTRIBUTING.md) (the rules, checks, CI
and licensing, which apply to agents too) and
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md). The decision records
(`docs/decisions/`, indexed in its `README.md`) say why things are as
they are; read the relevant one before changing a decision, and record a
new decision or pivot in a new one. The News API's quirks are in
`nextcloud-news`'s crate documentation, the sync algorithm in
`ren-sync`'s.

## Working

- One piece of work at a time, on a branch of its own. Open a draft pull
  request with the first push, so CI's full tests (`full-tests.yml`:
  Blitz, Servo's helper, the oldest supported Rust, the end-to-end test)
  run on it; they only run for pull requests, `master` and weekly. When
  it's done, summarise the result (including measurements and open
  issues), mark the pull request ready for review and stop for review.
- Keep semantically different changes in separate commits.
- Before committing: `just check-light`. Tests are mandatory.
- **Don't build Blitz or Servo unless the change needs it.** Build and
  test with `--no-default-features --features renderer-software,servo`
  (what `just check-light` does; the `servo` feature is only the window's
  side of the tabs). Only changes to the article view
  (`crates/ren/src/article/`, `ui/article.rs`) or the `html-view` feature
  need `just check`, and only changes to Servo's helper (`crates/ren-servo`,
  a workspace of its own) need `just check-servo`; even then, prefer
  letting CI's full tests build them over building Servo locally (12–24
  minutes per build).
- Task automation goes into the `justfile`; no Makefiles or `cargo xtask`.
- Low RAM and low CPU use are primary requirements. No async runtime, GPU
  stack or other heavy dependency without a reason in the commit message.
- Blitz stays behind the `html-view` feature; everything else builds and
  tests without it. Servo stays in the helper, which Ren starts with the
  first tab, never at startup.
- A new setting goes into the descriptor table (`ren-settings`'
  `schema.rs`) and into `docs/settings.md`, in the same order (a test
  checks it).
- New files get SPDX headers (`reuse annotate`) and `reuse lint` must
  pass; new dependencies must be on the list of compatible licences in
  `CONTRIBUTING.md` (`just licenses`).
- Commit messages and pull request descriptions must not link to agent
  sessions: the repository is public and the links are useless to anyone
  else.

## Measurements and real data

- Measurements that need a display or the real Nextcloud server are run
  by the maintainer. Prepare them as a single command (under `scripts/`,
  run through `just`) and say exactly what to run; don't invent numbers.
- Never commit real feed data or credentials. Data dumped from the server
  stays outside the repository until it's anonymised into test fixtures.

## Running Ren for testing

- Run it on a throwaway profile: the `XDG_CONFIG_HOME`, `XDG_STATE_HOME`,
  `XDG_DATA_HOME` and `XDG_CACHE_HOME` directories in a scratch
  directory. Its `settings.toml` must have no `[account]` section and set
  `[sync] interval-minutes = 0`: Ren syncs a few seconds after it
  starts, and a copy of the real settings would push test clicks (read
  marks) to the real account.
- To screenshot or send input to Ren's window, find it by the PID of the
  process you started (`_NET_WM_PID`), never by its title: other windows
  (a terminal in the repository, say) can be titled "ren" too.
