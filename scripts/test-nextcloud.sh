#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Runs the end-to-end test (crates/ren-sync/tests/nextcloud.rs: login,
# syncs, log out) against Nextcloud with the News app in a container, with
# SQLite. The container is made if it isn't running, and removed afterwards
# unless --keep is given (then the next run reuses it, which saves the
# minute Nextcloud takes to install). Needs docker or podman, and curl.
#
# Environment:
#   NEXTCLOUD_IMAGE  the image (default docker.io/library/nextcloud:32-apache)
#   NEWS_VERSION     the News release to install (default 28.7.0)
#   NEWS_APP         a directory with a built News app, instead of the
#                    release download
#   REN_E2E_PORT     the port on localhost for the server (default 8089)
#
# Usage: scripts/test-nextcloud.sh [--keep] [cargo test options]

set -euo pipefail

image=${NEXTCLOUD_IMAGE:-docker.io/library/nextcloud:32-apache}
news_version=${NEWS_VERSION:-28.7.0}
port=${REN_E2E_PORT:-8089}
name=ren-e2e-nextcloud
user=ren
# Only for this disposable server.
password=ren-e2e-password

keep=false
if [[ "${1:-}" == --keep ]]; then
    keep=true
    shift
fi

engine=$(command -v docker || command -v podman || true)
[[ -n "$engine" ]] || { echo "neither docker nor podman found" >&2; exit 1; }
command -v curl >/dev/null || { echo "curl not found" >&2; exit 1; }

occ() {
    "$engine" exec -u www-data "$name" php occ "$@"
}

if ! $keep; then
    trap '"$engine" rm -fv "$name" >/dev/null 2>&1 || true' EXIT
fi

if [[ "$("$engine" inspect -f '{{.State.Running}}' "$name" 2>/dev/null)" != true ]]; then
    "$engine" rm -fv "$name" >/dev/null 2>&1 || true
    echo "Starting $image"
    # The server fetches the test's feeds from this machine, by the name
    # host.docker.internal.
    "$engine" run -d --name "$name" -p "127.0.0.1:$port:80" \
        --add-host host.docker.internal:host-gateway \
        -e SQLITE_DATABASE=nextcloud \
        -e NEXTCLOUD_ADMIN_USER="$user" -e NEXTCLOUD_ADMIN_PASSWORD="$password" \
        -e NEXTCLOUD_TRUSTED_DOMAINS="localhost 127.0.0.1" \
        "$image" >/dev/null
fi

echo "Waiting for Nextcloud to install"
for _ in $(seq 300); do
    if curl -fsS "http://localhost:$port/status.php" 2>/dev/null | grep -q '"installed":true'; then
        break
    fi
    sleep 1
done
curl -fsS "http://localhost:$port/status.php" | grep -q '"installed":true' \
    || { "$engine" logs "$name" | tail -20; echo "Nextcloud didn't install" >&2; exit 1; }

if ! occ app:list --output=json | grep -q '"news":'; then
    echo "Installing News ${NEWS_APP:-$news_version}"
    if [[ -n "${NEWS_APP:-}" ]]; then
        tar -C "$(dirname "$NEWS_APP")" --exclude=.git --exclude=node_modules \
            --transform "s,^$(basename "$NEWS_APP"),news," -cf - "$(basename "$NEWS_APP")" \
            | "$engine" cp - "$name:/var/www/html/custom_apps/"
    else
        curl -fsSL "https://github.com/nextcloud/news/releases/download/$news_version/news.tar.gz" \
            | gzip -dc | "$engine" cp - "$name:/var/www/html/custom_apps/"
    fi
    "$engine" exec "$name" chown -R www-data:www-data /var/www/html/custom_apps/news
fi
occ app:enable news >/dev/null
# The test's feeds are on this machine, which Nextcloud refuses otherwise.
occ config:system:set allow_local_remote_servers --value true --type boolean >/dev/null
echo "News $(occ app:list --output=json | grep -o '"news":"[^"]*"' | cut -d'"' -f4)"

# A new app password per run: the test logs out with it, which revokes it.
# (Not with occ user:auth-tokens:add, which fails on Nextcloud 33.)
app_password=$(curl -fsS -u "$user:$password" -H 'OCS-APIRequest: true' \
    "http://localhost:$port/ocs/v2.php/core/getapppassword?format=json" \
    | grep -o '"apppassword":"[^"]*"' | cut -d'"' -f4)

REN_E2E_SERVER="http://localhost:$port" REN_E2E_USER=$user REN_E2E_PASSWORD=$app_password \
    cargo test --locked -p ren-sync --test nextcloud "$@" -- --ignored
