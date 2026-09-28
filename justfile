# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: CC0-1.0

# List the recipes
default:
    @just --list

# Run the checks CI runs
check:
    reuse lint
    cargo fmt --all --check
    cargo clippy --all-targets --locked -- -D warnings
    cargo test --locked
    cargo deny check licenses

# Measure startup, memory, idle and scrolling CPU per Slint renderer (needs a display)
measure *renderers:
    scripts/measure.sh {{renderers}}
