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
- Before committing: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`,
  and `cargo test` once tests exist. Tests are optional in phase 1 (spikes),
  mandatory from phase 2 on — but follow "Designing for testability" in the
  plan from the start.
- Task automation goes into a `justfile` (see "Conventions" in the plan);
  don't add Makefiles or `cargo xtask`.
- Low RAM and low CPU use are primary requirements. Don't add an async
  runtime, a GPU stack or other heavy dependencies without a reason stated in
  the commit message.
- Blitz (`html-view`) and Servo (`servo`) live behind cargo features;
  everything else must build and test without them. Servo must only ever be
  instantiated on demand, never at startup.
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
