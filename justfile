# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: CC0-1.0

# List the recipes
default:
    @just --list

# Run the checks CI runs
check:
    reuse lint
    cargo fmt --all --check
    cargo clippy --all-targets --locked --no-default-features --features renderer-software -- -D warnings
    cargo test --locked --no-default-features --features renderer-software
    cargo clippy --all-targets --locked -- -D warnings
    cargo test --locked
    cargo deny check licenses

# Measure startup, memory, idle and scrolling CPU per variant (needs a display)
measure *variants:
    scripts/measure.sh {{variants}}

# Measure the article view on items from a --dump-items directory (needs a display)
measure-articles dump *articles:
    scripts/measure-articles.sh {{dump}} {{articles}}

# Measure memory and frames of web page tabs, with pages from a --dump-items directory or the given URLs (needs a display)
measure-tabs dump *urls:
    scripts/measure-tabs.sh {{dump}} {{urls}}

# Survey the HTML and CSS used by the articles in a --dump-items directory
survey-articles dump:
    scripts/survey-articles.py {{dump}}

# Measure fetching all unread items per batch size (needs the real server)
measure-fetch *batch_sizes:
    scripts/measure-fetch.sh {{batch_sizes}}
