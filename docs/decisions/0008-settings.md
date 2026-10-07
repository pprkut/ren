<!--
SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
SPDX-License-Identifier: GPL-3.0-or-later
-->

# 0008 — Settings, UI state and the app password (M4)

**Status:** done (2026-10-07), for review; checked with KWallet on
Plasma.

## Context

`docs/PLAN.md` ("Settings, state and credentials") keeps three kinds of
data apart: the settings, curated by the user in
`$XDG_CONFIG_HOME/ren/settings.toml`; the UI state in
`$XDG_STATE_HOME/ren/state.toml`, written by ren alone; and the app
password, never in a file. Until now the S2 reader in `ren` read
`[account]` with serde and took the password from `REN_APP_PASSWORD` or
`password-command`. M4 builds the settings model, the state file and the
secret store; the settings dialog and the login are M8b.

## What was built

A new crate, `ren-settings`, with no UI and no knowledge of the server.
`ren` uses it for the command line modes.

- **Descriptors** (`schema.rs`): one static table of settings with key
  (`table.key`), title, description, category and kind: switch, choice
  (value and title per option), number (range, unit), text (an optional
  check) or colour (`#rrggbb`), each with its default. Loading,
  validation, writing and later the dialog all go by it. The table has
  what ren uses or the plan names now: `account.server` (checked: https,
  or http on the local machine), `account.user`,
  `account.password-command`, `sync.interval-minutes` (15; 0 syncs only
  when asked) and `sync.keep-read-days` (30; 0 keeps everything). The
  other kinds are tested with a table of their own.
- **Loading** (`Settings::parse`): the text is parsed with `toml_edit`
  (spans kept) and walked against the table. Unknown tables and keys are
  warnings (typos, or settings of a newer ren); a syntax error, a wrong
  type, a value out of range or a failed check are errors, and then none
  of the file is used (half a file can mean something else than the
  whole: a server without its user). Every problem has line and column
  (characters, counted from 1) and prints as `line:column: message`, so
  `file:line:column: message` works as a jump target in editors. Dotted
  keys and inline tables are read too. Values not in the file are the
  defaults; empty text counts as unset.
- **Writing** (`set`, `reset`): one value is changed in the parsed
  document (`DocumentMut`), everything else is written back as it was:
  comments, order, blank lines, quoting. The comment after a value stays
  with the new value. Setting a value to its default removes the key
  (the file only holds overrides); a table left empty goes too, unless a
  comment is attached to it. A file with errors is not written; the user
  fixes it first.
- **The file** (`SettingsFile`): open (a missing file is the defaults),
  `reload` when the file changed, `set` and `reset`. A change is seen by
  a stamp of modification time, size and inode, without reading the
  file; the app calls `reload` when its window gains focus (M5), instead
  of watching the file with inotify (no thread, no dependency; edits in
  another editor apply when the user comes back). After an error the
  last good settings stay in effect. Files are written through a
  temporary file next to the target and a rename, following a symbolic
  link (dotfiles kept elsewhere) and keeping the permissions.
- **State file** (`State`): sort order, collapsed folders (so new ones
  are expanded), the last selection (list and item, by server id), and
  per arrangement the tree width, the list's size and the column widths.
  All optional; read and written whole with serde, since only ren writes
  it. A file that can't be read is the empty state plus a message, never
  a reason not to start. M8 fills it.
- **Secret store** (`secret.rs`): a `SecretStore` trait (get, set,
  delete the app password of an account), `SecretService` (below), and
  `MemoryStore`, a fake that can also fail on demand. `app_password`
  takes the first of `REN_APP_PASSWORD`, `password-command` (if set) and
  the secret store, which it only connects to then. Its errors tell
  apart "not stored" (log in), "no Secret Service" (use
  `password-command`), "denied" (the user didn't unlock the keyring)
  and a failing command.
- **Paths**: `$XDG_CONFIG_HOME` and `$XDG_STATE_HOME` with the home
  directory fallbacks; relative values are ignored, as the specification
  says. The environment is passed in.
- **In `ren`**: the server commands load the settings through
  `SettingsFile` (warnings to stderr, errors stop with
  `file:line:column`), get the password from `app_password`, and `--sync`
  purges after `sync.keep-read-days`. New until M8b's account page:
  `--set-password` reads the app password from stdin (no echo on a
  terminal, via `stty`), checks it with the server and stores it;
  `--check-secret-service` names the program that answers the Secret
  Service, stores, reads and removes a test password, says whether the
  account's app password is stored, and prints what connecting cost.
  `just check-secret-service` runs it. The S2 reader is gone, and with
  it `ren`'s own dependency on `toml`.

### Choices and findings

- **Which keyring store reuses Slint's zbus.** keyring 4 is split into
  `keyring-core` and one crate per store. On Linux there are two Secret
  Service stores: `zbus-secret-service-keyring-store` (through the
  `secret-service` crate on zbus 5) and
  `dbus-secret-service-keyring-store` (libdbus, a C library and its
  development files). Slint's winit backend already links zbus 5.19 on
  the async-io runtime, so ren uses the zbus store with
  `rt-async-io-crypto-rust`: one zbus in the build, no new runtime.
  `ren-settings` depends on `keyring-core` and the store directly, not
  on the `keyring` crate, whose default features add macOS and Windows
  stores. New crates: `keyring-core`, `zbus-secret-service-keyring-store`,
  `secret-service`, `num`, `num-iter` (all MIT OR Apache-2.0); the
  RustCrypto crates for the encrypted session were already in the lock
  file. `ren-settings` also uses zbus directly, for the name of the
  program that owns `org.freedesktop.secrets`.
- **Encrypted session.** The store always opens a Diffie-Hellman
  session (`EncryptionType::Dh`), as libsecret does, so the password
  isn't plain on the bus. crypto-rust instead of OpenSSL: no C library.
- **Attributes.** `service` = `ren`, `username` = `<user>@<server>`, and
  a label naming the account ("ren: Nextcloud News app password for …"),
  which is what Seahorse and KWallet Manager show. The fixed `service`
  keeps ren's items apart from those of other programs (the Nextcloud
  desktop client stores its own app passwords); the server in the user
  name keeps accounts on different servers apart. A password can be
  stored by hand with `secret-tool store --label=ren service ren username
  user@https://cloud.example.org`; ren reads it (tried with GNOME
  Keyring).
- **Blocking calls.** The Secret Service calls block, also while the
  desktop asks to unlock the keyring. They belong on the sync thread
  (M5), never on the UI thread; connecting only when the password is
  needed (`app_password`'s closure) keeps the D-Bus connection out of
  runs that use `password-command`.
- **Precedence of `password-command`.** When it's set, the Secret
  Service isn't asked: the setting is an explicit choice, and asking a
  Secret Service that isn't there would only add a delay and an error.
- **One value at a time, through `toml_edit`.** The settings model never
  serialises the whole settings file. `toml_edit` was already compiled
  for the build (Slint and zbus use it through `proc-macro-crate`), now
  also for ren itself.
- **Reload by stamp, not inotify.** A focus-in check is enough for a
  file the user edits by hand. A rewrite with the same size within the
  file system's time resolution would go unnoticed; editors that save
  through a new file (most) also change the inode.

## Testing

- 40 unit tests in `ren-settings`: parsing of every kind, defaults,
  warnings and errors with their positions (also after multi-byte
  characters), dotted keys and inline tables, edits that keep comments
  and order, setting the default and resetting, round trips through the
  app's table, reloading after changes, removal and errors (the last
  good settings stay), writing through symbolic links with permissions
  kept, the state file, the XDG paths, the fake store and the password
  precedence (environment, command, store; each failure).
- `tests/secret_service.rs` runs the real Secret Service implementation:
  store, replace, read, delete, two accounts apart. It's ignored by
  default; `just test-secret-service` (`scripts/test-secret-service.sh`)
  runs it on a private session bus with GNOME Keyring and a new keyring
  in a temporary directory, so the user's keyring is never touched. CI
  runs it in the light job.
- By hand, in the container with GNOME Keyring: `--check-secret-service`
  with nothing stored, with a password stored by `secret-tool`, `--check`
  with and without a stored password, no Secret Service on the bus (the
  error suggests `password-command`), no session bus, and settings files
  with errors and warnings.

## Measurements

`ren --check-secret-service` (release build without the rendering
engines) against GNOME Keyring 46 in the build container, three runs:
connecting takes 7.5–7.7 ms and raises RSS from 8.4–8.6 MiB to
9.5–9.6 MiB; with the test password stored, read and removed and the
app password read, RSS ends at 9.9–10.1 MiB. That's the cost of the
D-Bus connection (zbus with its executor thread, the session) while the
password is fetched; the app drops the connection afterwards, which M5
should check in the running app.

The release binary without the rendering engines grows from 22.3 MiB
(master) to 23.4 MiB (+1.1 MiB: the Secret Service client and its
crypto, `toml_edit` for ren itself). Code that isn't run isn't loaded,
so this is no RSS.

On Plasma, `just check-secret-service` reports `Secret Service:
ksecretd (pid …)`: KWallet answers the Secret Service API. Newer KWallet
releases run it as `ksecretd` instead of `kwalletd6`.

## Open

- **Settings in the app** are M5 (load at start, reload on focus) and
  M8b (the dialog, with the descriptors; arrangement and item colours
  join the table then). `sync.interval-minutes` waits for M8's periodic
  sync; 0007 planned `resync_after` as a setting too, which isn't one
  yet (30 days fits).
- **JSON Schema** for TOML editors, generated from the descriptors, is
  optional in the plan and not done.
- **Asking at every start** when there is neither a Secret Service nor a
  `password-command` needs a dialog: M8b, with the login.
