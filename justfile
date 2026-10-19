# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: CC0-1.0

name := 'ren'
appid := 'io.github.pprkut.ren'

# Where `just install` puts Ren, set by name as in COSMIC's justfiles:
# `just prefix=/usr rootdir="$PKG" install` for packages (rootdir is
# prepended to every path), `just prefix="$HOME/.local" install` for
# yourself.
rootdir := ''
prefix := '/usr/local'
docdir := prefix / 'share' / 'doc' / name
# `just helper=false install` leaves out the Servo helper: web pages then
# open in the browser.
helper := 'true'

base-dir := absolute_path(clean(rootdir / prefix))
doc-dst := absolute_path(clean(rootdir / docdir))
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')
bin-src := cargo-target-dir / 'release' / name
bin-dst := base-dir / 'bin' / name
helper-src := cargo-target-dir / 'release' / 'ren-servo'
# Where ren looks for it: ../libexec/ren/ from its binary.
helper-dst := base-dir / 'libexec' / name / 'ren-servo'
desktop-dst := base-dir / 'share' / 'applications' / appid + '.desktop'
metainfo-dst := base-dir / 'share' / 'metainfo' / appid + '.metainfo.xml'
icons-dst := base-dir / 'share' / 'icons' / 'hicolor'

# List the recipes
default:
    @just --list

# Run the checks CI runs, except for Servo's helper (check-servo)
check: reuse typos editorconfig validate-data fmt-check machete clippy-light test-light clippy test licenses advisories

# Run the checks without Blitz and Servo (fast; CI's pull request jobs build them)
check-light: reuse fmt-check clippy-light test-light
    @if command -v typos >/dev/null; then typos; else echo "typos not installed; CI checks the spelling"; fi
    @if command -v cargo-deny >/dev/null; then {{just_executable()}} licenses; else echo "cargo-deny not installed; CI checks the licenses"; fi

# Check the REUSE compliance (licences and copyright of every file)
reuse:
    reuse lint

# Look for misspelt words in the code and the docs (typos-cli; typos.toml)
typos:
    typos

# Check the files against .editorconfig: line ends, final newlines, trailing whitespace, tabs (editorconfig-checker)
editorconfig:
    git ls-files --exclude-standard -z | xargs -0 "$(command -v editorconfig-checker || echo ec)"

# Validate the .desktop file and the AppStream metadata (desktop-file-utils, appstream)
validate-data:
    desktop-file-validate data/{{appid}}.desktop
    appstreamcli validate --pedantic data/{{appid}}.metainfo.xml

# Check the formatting of both workspaces
fmt-check:
    cargo fmt --all --check
    cd crates/ren-servo && cargo fmt --check

# Look for unused dependencies in both workspaces (cargo-machete)
machete:
    cargo machete

# Lint without Blitz and Servo: without and with the window's side of the tabs
clippy-light:
    cargo clippy --all-targets --locked --no-default-features --features renderer-software -- -D warnings
    cargo clippy --all-targets --locked --no-default-features --features renderer-software,servo -- -D warnings

# Test without Blitz and Servo
test-light:
    cargo test --locked --no-default-features --features renderer-software,servo

# Lint with all default features, including the article view (builds Blitz)
clippy:
    cargo clippy --all-targets --locked -- -D warnings

# Test with all default features, including the article view (builds Blitz)
test:
    cargo test --locked

# Check that ren and its Servo helper build with their rust-version, the oldest Rust they support (needs rustup; builds Blitz and Servo)
msrv:
    #!/usr/bin/env bash
    set -euo pipefail
    for dir in . crates/ren-servo; do
        version=$(sed -n 's/^rust-version = "\(.*\)"/\1/p' "$dir/Cargo.toml")
        echo "$dir: Rust $version"
        (cd "$dir" && cargo "+$version" check --locked)
    done

# Check the licences of ren's dependencies (cargo-deny)
licenses:
    cargo deny check licenses

# Check ren's dependencies for security advisories, unmaintained and yanked crates (cargo-deny)
advisories:
    cargo deny check advisories

# Check the desktop's Secret Service (e.g. KWallet on Plasma) with a test password, and whether the app password is stored
check-secret-service:
    cargo run --release --locked --no-default-features --features renderer-software -- --check-secret-service

# Test the app password in the Secret Service, with GNOME Keyring on a private session bus
test-secret-service *args:
    scripts/test-secret-service.sh {{args}}

# Test login, syncs and log out against Nextcloud with News in a container (docker or podman); --keep keeps it for the next run
test-nextcloud *args:
    scripts/test-nextcloud.sh {{args}}

# Run the checks of CI's Servo job: the helper, ren-servo (a long build)
check-servo: fmt-check clippy-servo test-servo
    @if command -v cargo-deny >/dev/null; then {{just_executable()}} licenses-servo advisories-servo; else echo "cargo-deny not installed; CI checks the licenses"; fi

# Lint Servo's helper (builds Servo)
clippy-servo:
    cd crates/ren-servo && cargo clippy --all-targets --locked -- -D warnings

# Test Servo's helper (builds Servo)
test-servo:
    cd crates/ren-servo && cargo test --locked

# Check the licences of the Servo helper's dependencies (cargo-deny)
licenses-servo:
    cargo deny --manifest-path crates/ren-servo/Cargo.toml check licenses

# Check the Servo helper's dependencies for security advisories, unmaintained and yanked crates (cargo-deny)
advisories-servo:
    cargo deny --manifest-path crates/ren-servo/Cargo.toml check advisories

# Build ren and its Servo helper (ren-servo, next to ren in target/), for web pages in tabs
build profile="release":
    cargo build --locked --profile {{profile}}
    cd crates/ren-servo && cargo build --locked --profile {{profile}}

# Generate THIRD-PARTY-NOTICES.html from both workspaces' crates (cargo-about) and notices/; --check compares instead
notices *args:
    scripts/notices.sh {{args}}

# Install what `just build` built: the binary, the Servo helper in libexec/ren, the .desktop file, AppStream, icons, docs
install:
    install -Dm0755 {{bin-src}} {{bin-dst}}
    if [ '{{helper}}' = true ]; then install -Dm0755 {{helper-src}} {{helper-dst}}; fi
    install -Dm0644 data/{{appid}}.desktop {{desktop-dst}}
    install -Dm0644 data/{{appid}}.metainfo.xml {{metainfo-dst}}
    install -Dm0644 crates/ren/icons/ren.svg {{icons-dst}}/scalable/apps/{{appid}}.svg
    for size in `ls data/icons`; do \
        install -Dm0644 data/icons/$size/{{appid}}.png {{icons-dst}}/$size/apps/{{appid}}.png; \
    done
    install -Dm0644 README.md {{doc-dst}}/README.md
    install -Dm0644 THIRD-PARTY-NOTICES.html {{doc-dst}}/THIRD-PARTY-NOTICES.html
    install -Dm0644 LICENSES/GPL-3.0-or-later.txt {{doc-dst}}/COPYING

# Remove what `just install` installed (with the same variables)
uninstall:
    rm -f {{bin-dst}} {{helper-dst}} {{desktop-dst}} {{metainfo-dst}}
    rm -f {{icons-dst}}/scalable/apps/{{appid}}.svg {{icons-dst}}/*/apps/{{appid}}.png
    rm -f {{doc-dst}}/README.md {{doc-dst}}/THIRD-PARTY-NOTICES.html {{doc-dst}}/COPYING
    -rmdir {{doc-dst}} {{base-dir}}/libexec/{{name}}

# Vendor both workspaces' crates into vendor.tar.xz, for building without the network (packages; see build-vendored)
vendor:
    #!/usr/bin/env bash
    set -euo pipefail
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    mkdir "$work/.cargo"
    # One directory for both workspaces; versions in the names, as the
    # two lock files may want different versions of a crate. The last
    # line of cargo's configuration has vendor/'s absolute path.
    cargo vendor --locked --versioned-dirs --sync crates/ren-servo/Cargo.toml "$work/vendor" \
        | head -n -1 > "$work/.cargo/config.toml"
    echo 'directory = "vendor"' >> "$work/.cargo/config.toml"
    tar -C "$work" -cf - .cargo vendor | xz -T0 > vendor.tar.xz
    ls -l vendor.tar.xz

# Unpack vendor.tar.xz into the source: .cargo/config.toml and vendor/
vendor-extract:
    rm -rf vendor
    tar -xf vendor.tar.xz

# Build like `just build`, from vendor.tar.xz without the network (MOZJS_FROM_SOURCE=1 for SpiderMonkey)
build-vendored: vendor-extract
    cargo build --frozen --release
    cd crates/ren-servo && cargo build --frozen --release

# Remove what vendor and vendor-extract made
clean-vendor:
    rm -rf .cargo vendor vendor.tar.xz

# Render the logo's PNGs for the icon theme into data/icons (needs rsvg-convert)
icons:
    #!/usr/bin/env bash
    set -euo pipefail
    # The small logo up to 32 pixels, the logo with its wires from 48.
    for size in 16 22 24 32 48 64 128 256 512; do
        if ((size < 48)); then svg=ren-small.svg; else svg=ren.svg; fi
        mkdir -p data/icons/${size}x$size
        rsvg-convert -w $size -h $size crates/ren/icons/$svg -o data/icons/${size}x$size/{{appid}}.png
    done

# Measure startup, memory, idle and scrolling CPU per variant (needs a display)
measure *variants:
    scripts/measure.sh {{variants}}

# Measure the article view on items from a --dump-items directory (needs a display)
measure-articles dump *articles:
    scripts/measure-articles.sh {{dump}} {{articles}}

# Measure memory and frames of web page tabs, with pages from a --dump-items directory or the given URLs (needs a display)
measure-tabs dump *urls:
    scripts/measure-tabs.sh {{dump}} {{urls}}

# Load one page again and again, each in a new Servo helper: frames, requests and the GPU's clock per load; BUSY=n keeps n cores busy in a first phase (needs a display)
measure-page-loads dump url:
    scripts/measure-page-loads.sh {{dump}} {{url}}

# Measure memory with the window hidden in the tray, with the articles of a --dump-items directory and the given pages in tabs (needs a display)
measure-hidden dump *urls:
    scripts/measure-hidden.sh {{dump}} {{urls}}

# Measure memory with the settings dialog and About ren open and after closing them (needs a display)
measure-settings:
    scripts/measure-settings.sh

# Survey the HTML and CSS used by the articles in a --dump-items directory
survey-articles dump:
    scripts/survey-articles.py {{dump}}

# Turn a --dump-items directory into anonymised test fixtures for nextcloud-news
anonymise-dump dump *out:
    scripts/anonymise-dump.py {{dump}} {{out}}

# Measure the store with a synthetic account: writing, list queries, purging
measure-store items="63000":
    mkdir -p target/measure
    cargo run --release --locked -p ren-store --example measure -- {{items}} target/measure

# Measure syncing into a new database: initial, incremental, resync (needs the real server)
measure-sync:
    scripts/measure-sync.sh

# Measure syncing in the window: a full and an incremental sync into a new database (needs a display and the real server)
measure-window-sync:
    scripts/measure-window-sync.sh

# Measure fetching all unread items per batch size (needs the real server)
measure-fetch *batch_sizes:
    scripts/measure-fetch.sh {{batch_sizes}}

# Run all measurements one after another and collect them in one report (needs a display, network and the real server; about an hour)
measure-all dump *urls:
    scripts/measure-all.sh {{dump}} {{urls}}
