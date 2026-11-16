<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Contributing to Ren

How Ren is built is in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md),
what was decided and measured on the way in
[`docs/decisions/`](docs/decisions/README.md), what Ren costs in
[`docs/performance.md`](docs/performance.md). Building is in the
[README](README.md).

## Changes

Known bugs and ideas not yet worked on are
[GitHub issues](https://github.com/pprkut/ren/issues).

- Keep semantically different changes in separate commits.
- Low memory and processor use come first. A new heavy dependency
  (anything that brings an async runtime, a GPU stack and the like) needs
  its reason in the commit message; prefer small, well-maintained ones.
- Tests come with the change. Follow "Designing for testability" in the
  architecture: logic in plain Rust outside the UI, seams as small traits,
  no hidden global state or I/O.
- A change that costs memory or CPU is measured (see "Measuring" in
  `docs/performance.md`), an upgrade of Blitz or Servo on its own.
- A decision worth keeping (a choice between alternatives, a pivot, a
  measurement that settles a question) gets a decision record in
  `docs/decisions/`: the next number, `NNNN-<topic>.md`, with context,
  what was tried, measurements, the decision and its consequences, and a
  line in its `README.md`.
- Never commit real feed data or credentials. Data dumped from a server
  (`ren --dump-items`) stays outside the repository until it is
  anonymised into test fixtures (`just anonymise-dump`).

## Checks

`just` lists the tasks. Before committing:

- `just check-light`: `reuse lint`, `cargo fmt`, clippy with `-D
  warnings` and the tests without Blitz and Servo, and typos and `cargo
  deny` if they're installed. Fast; enough for most changes.
- `just check`: everything CI runs but Servo's helper; builds Blitz.
  Needed for changes to the article view (`crates/ren/src/article/`,
  `ui/article.rs`) or the `html-view` feature.
- `just check-servo`: Servo's helper (`crates/ren-servo`). A build of
  Servo takes 12–24 minutes; CI's full tests run it on pull requests.

Without the engines, build and test with `--no-default-features
--features renderer-software,servo` (the `servo` feature is only the
window's side of the tabs).

## CI

The workflows (`.github/workflows/`) are split by when they run, and
their jobs run the `justfile`'s recipes, so `just check` runs what CI
runs:

- `general.yml`, on every push: REUSE, typos, EditorConfig, shellcheck,
  zizmor (the workflows), the `.desktop` file and the AppStream metadata.
- `dependencies.yml`, on every push and weekly: both workspaces'
  licences and RustSec advisories (cargo-deny, policy in `deny.toml`),
  unused dependencies (cargo-machete), the third-party notices.
- `code-tests.yml`, on every push and weekly: `cargo fmt`, clippy and the
  tests without Blitz and Servo, the Secret Service test.
- `full-tests.yml`, for pull requests, `master`, weekly and on demand:
  clippy and the tests with all default features (under cargo-nextest,
  with coverage, both uploaded to Codecov), Servo's helper, both
  workspaces with their `rust-version`, the end-to-end test against
  Nextcloud.
- `performance.yml`, for pull requests and `master`: the measurements
  that are steady on GitHub's runners (`just measure-ci`), uploaded to
  Bencher by `performance-upload.yml`; `performance-closed.yml` archives a
  closed pull request's branch there.

Keep it green. The default builds stay fast: the engines are only built
in `full-tests.yml` and `performance.yml`, which run for pull requests,
so work on a branch gets a draft pull request with its first push and is
marked ready for review when it's done. A new check gets a recipe of its
own, which CI runs. Actions are pinned by commit (dependabot updates
them weekly), jobs ask for their permissions, Rust jobs set up with
`.github/actions/setup-rust` (build caches are saved on `master` only),
and zizmor must pass. The design of CI is in
[0019](docs/decisions/0019-ci.md).

## Task automation

Tasks go into the `justfile` at the root ([just](https://just.systems/)):
no Makefiles, `cargo xtask` or ad-hoc wrapper scripts. Cargo stays the
build system; recipes wrap tasks of several steps. Scripts too long for a
recipe live in `scripts/` and are run from one.

## Licensing

Ren is licensed under **GPL-3.0-or-later**, trivial files under
**CC0-1.0**, and follows the [REUSE specification](https://reuse.software/).

- Every file carries SPDX headers in this form, in the file type's
  comment syntax (copy them from an existing file, or `reuse annotate`):

  ```
  # SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
  # SPDX-License-Identifier: GPL-3.0-or-later
  ```

  Trivial files (build and tool configuration such as `Cargo.toml`,
  `.gitignore`, `REUSE.toml`, CI workflows, and generated files like
  `Cargo.lock`) use `CC0-1.0`. Files that can't carry a header (generated
  files, binary assets, JSON) are annotated in `REUSE.toml`. Third-party
  assets keep their licence; its text goes to `LICENSES/`.
- `reuse lint` must pass before each commit.
- Dependencies must be compatible with GPL-3.0-or-later. Acceptable: MIT,
  Apache-2.0 (also with LLVM-exception), BSD-2-Clause, BSD-3-Clause, ISC,
  Zlib, BSL-1.0, Unicode-3.0, CC0-1.0, MPL-2.0 (unless marked
  "Incompatible With Secondary Licenses"), LGPL-2.1-or-later, LGPL-3.0,
  GPL-3.0. Slint is used under its `GPL-3.0-only` option, so the combined
  binary is distributed under GPLv3. Not acceptable: GPL-2.0-only,
  proprietary or non-commercial licences, and anything unclear.
  `deny.toml` encodes the list (`just licenses`, `just licenses-servo`).
- Exceptions are decided one crate at a time and listed in `deny.toml`
  with their reason: `webpki-roots` (CDLA-Permissive-2.0, Mozilla's CA
  certificates as data, pulled in by Servo) and `ksni` (Unlicense, the
  tray icon's client behind Slint).
- `THIRD-PARTY-NOTICES.html` lists the licences of what Ren is built
  from; `just notices` regenerates it when dependencies change, and CI
  checks that it matches.
