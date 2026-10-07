#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Runs the Secret Service tests of ren-settings against GNOME Keyring on a
# private session bus, with a new keyring in a temporary directory, so the
# user's keyring is never touched. Needs dbus-run-session (dbus) and
# gnome-keyring-daemon (gnome-keyring).
#
# Usage: scripts/test-secret-service.sh [cargo test options]

set -euo pipefail

if [[ "${1:-}" != --inside ]]; then
    for tool in dbus-run-session gnome-keyring-daemon; do
        command -v "$tool" >/dev/null || { echo "$tool not found" >&2; exit 1; }
    done
    exec dbus-run-session -- "$0" --inside "$@"
fi
shift

data=$(mktemp -d)
trap 'kill "${GNOME_KEYRING_PID:-}" 2>/dev/null || true; rm -rf "$data"' EXIT
export XDG_DATA_HOME=$data
# A new login keyring with the password "test", unlocked; it becomes the
# default collection.
eval "$(printf test | gnome-keyring-daemon --unlock --components=secrets)"
export GNOME_KEYRING_PID

cargo test --locked -p ren-settings --test secret_service "$@" -- --ignored --nocapture
