<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# ren

Native Nextcloud News reader in Rust (Slint UI, Blitz article view,
on-demand Servo tabs for full pages).

- The roadmap, architecture, API notes and sync algorithm are in
  `docs/PLAN.md`. The technology choices there are provisional until the
  phase 1 spikes confirm them; decisions and pivots go to `docs/decisions/`.
- Work on **one milestone at a time**, in the order of `docs/PLAN.md`. When
  a milestone is complete, mark it done in the plan, summarise the result
  (including measurements and open issues) and stop for review. Don't start
  the next milestone without the go-ahead.
- Keep semantically different changes in separate commits.
- Open a draft pull request for the milestone's branch with the first
  push, so CI's full job (with `html-view`, later `servo`) runs on it, and
  mark it ready for review when the milestone is done. The full job only
  runs for pull requests, `master` and weekly; a branch without a pull
  request is only checked without the rendering engines.
- Before committing: `just check-light` (`reuse lint`, `cargo fmt`, clippy
  and tests without the rendering engines, `cargo deny` if installed).
  Tests are optional in phase 1 (spikes), mandatory from phase 2 on — but
  follow "Designing for testability" in the plan from the start.
- **Don't build Blitz or Servo unless the change needs it.** Build and test
  with `--no-default-features --features renderer-software,servo` (that's
  what `just check-light` does; the `servo` feature is only the window's
  side of the tabs). Only changes to the article view (`article/`,
  `ui/article.rs`) or to the `html-view` feature need the full build (`just
  check`), and only changes to Servo's helper (`crates/ren-servo`, a
  workspace of its own) need `just check-servo`; even then, prefer letting
  CI's pull request jobs run them over building Servo locally (12–24
  minutes per build). Phase 2 (M1–M4) never needs them.
- Task automation goes into a `justfile` (see "Conventions" in the plan);
  don't add Makefiles or `cargo xtask`.
- Low RAM and low CPU use are primary requirements. Don't add an async
  runtime, a GPU stack or other heavy dependencies without a reason stated in
  the commit message.
- Blitz lives behind the `html-view` cargo feature; everything else must
  build and test without it. Servo lives in its own program, the helper
  `ren-servo` (`crates/ren-servo`, its own workspace and `Cargo.lock`:
  Servo's and Blitz's stylo versions can't share a build), which ren starts
  on demand, never at startup; `just build` builds both.
- Measurements that need a display or the real Nextcloud server are run by
  the user locally. Prepare them as a single command (under `scripts/`) and
  say exactly what to run; don't invent numbers.
- Licensing: GPL-3.0-or-later, trivial files (build/tool config, CI,
  generated files) CC0-1.0; REUSE compliant. Every new file gets SPDX
  headers (`reuse annotate`, see "Licensing" in the plan) and `reuse lint`
  must pass before committing. New dependencies must be on the plan's list
  of compatible licenses (checked by `cargo deny check licenses` once
  `deny.toml` exists).
- Never commit real feed data or credentials. Data dumped from the server
  stays outside the repository until it has been anonymised into test
  fixtures.
