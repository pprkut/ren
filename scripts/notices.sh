#!/bin/bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Generates THIRD-PARTY-NOTICES.html: the licences of the crates Ren and
# its Servo helper are built from (cargo-about's JSON for both
# workspaces, merged with jq: cargo-about's templates take one workspace,
# and the two can't be one), the libraries vendored inside crates
# (SpiderMonkey's, in mozjs_sys), and what isn't a crate
# (notices/header.html). Needs cargo-about and jq.
#
#     scripts/notices.sh [--check]
#
# --check compares with the committed file instead of writing it (CI).
# Offline after `cargo fetch` (so the result doesn't depend on the
# network): crates that lack their licence file get the licence's
# standard text.

set -euo pipefail
cd "$(dirname "$0")/.."
root=$PWD
output=THIRD-PARTY-NOTICES.html
workspaces=(Cargo.toml crates/ren-servo/Cargo.toml)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

for manifest in "${workspaces[@]}"; do
    cargo fetch --locked --manifest-path "$manifest"
    (cd "$(dirname "$manifest")" \
        && cargo about generate --format json --all-features --frozen -c "$root/about.toml") \
        >> "$work/reports.json"
done

# Libraries inside mozjs_sys (SpiderMonkey's source), which cargo-about
# can't see: name, what it is, and its licence file, in the crate
# (mozjs_sys:) or in the repository.
mozjs=$(cargo metadata --format-version 1 --frozen --manifest-path crates/ren-servo/Cargo.toml \
    | jq -r '.packages[] | select(.name == "mozjs_sys") | .manifest_path')
mozjs=$(dirname "$mozjs")
while IFS='|' read -r title about path; do
    path=${path/#mozjs_sys:/$mozjs/}
    jq -n --arg title "$title" --arg about "$about" --rawfile text "$path" \
        '{$title, $about, $text}' >> "$work/vendored.json"
done <<'VENDORED'
ICU|Unicode's internationalisation library, in SpiderMonkey for Intl and Unicode data (ICU 78).|notices/icu-78.txt
irregexp|V8's regular expression engine, in SpiderMonkey.|mozjs_sys:mozjs/js/src/irregexp/LICENSE.v8
double-conversion|Conversions between numbers and text, in SpiderMonkey.|mozjs_sys:mozjs/mfbt/double-conversion/LICENSE
fdlibm|The math functions of SpiderMonkey.|mozjs_sys:mozjs/modules/fdlibm/src/LICENSE
LZ4|Compression, in SpiderMonkey's support library (mozglue).|mozjs_sys:mozjs/mozglue/static/lz4/LICENSE
function2|Function wrappers, in SpiderMonkey's support code.|mozjs_sys:mozjs/third_party/function2/LICENSE.txt
VENDORED

# Each licence text once (compared without whitespace), with the crates
# using it: not Ren's own, which come from paths, not a registry.
program=$(cat <<'JQ'
# Quotes too, except in <pre> (where they're most of the escapes).
def escape: gsub("&"; "&amp;") | gsub("<"; "&lt;") | gsub(">"; "&gt;");
def escape_all: escape | gsub("\""; "&quot;") | gsub("'"; "&#x27;");
def pre:
    "<pre>"
    + (sub("^\n+"; "") | sub("\n+$"; "") | split("\n") | map(sub("\\s+$"; "")) | join("\n") | escape)
    + "</pre>";

[
    $reports[].licenses[]
    | {id, name, text, users: [.used_by[].crate | select(.source != null) | [.name, .version]]}
    | select(.users != [])
    | .same = [.id, ([.text | splits("\\s+")] | map(select(. != "")) | join(" "))]
]
# Texts of one licence with the same first crate in the reports' order.
| to_entries
| map(.value + {order: .key})
| group_by(.same)
| map(.[0] + {users: (map(.users[]) | unique)})
| sort_by(.id, .users[0], .order) as $entries
| ($header | sub("\\s+$"; "")),
  "",
  "<h2>Libraries inside crates</h2>",
  "<p>SpiderMonkey, Servo's JavaScript engine, comes with libraries of its own in the crate mozjs_sys (listed below under its licence, the MPL 2.0):</p>",
  ($vendored[] | "<h3>\(.title | escape_all)</h3>", "<p>\(.about | escape_all)</p>", (.text | pre)),
  "",
  "<h2>Rust crates</h2>",
  "<p>The \([$entries[].users[]] | unique | length) crates Ren and its web page helper are built from, by licence text.</p>",
  ($entries[]
   | "<h3>\(.name | escape_all)</h3>",
     "<p>Used by \(.users | map(join(" ")) | join(", ") | escape_all).</p>",
     (.text | pre))
JQ
)
jq -nr --rawfile header notices/header.html \
    --slurpfile reports "$work/reports.json" --slurpfile vendored "$work/vendored.json" \
    "$program" > "$work/$output"

if [[ "${1:-}" == --check ]]; then
    cmp -s "$work/$output" "$output" \
        || { echo "$output doesn't match Cargo.lock: run \`just notices\`" >&2; exit 1; }
    echo "$output is up to date"
else
    mv "$work/$output" "$output"
    echo "$output: $(( $(stat -c %s "$output") / 1024 )) KiB"
fi
