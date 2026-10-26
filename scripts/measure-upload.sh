#!/usr/bin/env bash
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Uploads the results of `just measure-all` to Bencher, as the desktop's
# testbed (docs/decisions/0019-ci.md, "Performance tracking"): the merged
# BMF results of its directory, for the commit it measured. Needs the
# Bencher CLI (`bencher`) and an API key of the project.
#
# Usage: scripts/measure-upload.sh DIR
#   DIR: the output directory of a `just measure-all` run, with
#        results.json and commit
#
# Environment:
#   BENCHER_API_KEY  the key (required)
#   BENCHER_PROJECT  the project (default: ren)
#   TESTBED          the testbed (default: desktop)
#   BRANCH           the branch the commit is on (default: master)

set -euo pipefail
cd "$(dirname "$0")/.."

if [[ $# -ne 1 ]]; then
    sed -n 's/^# \?//p' "$0" | sed -n '4,18p' >&2
    exit 1
fi
dir=$1
if [[ -z ${BENCHER_API_KEY:-} ]]; then
    echo "BENCHER_API_KEY is not set" >&2
    exit 1
fi
if [[ ! -s $dir/results.json ]]; then
    echo "$dir/results.json: no results" >&2
    exit 1
fi
commit=$(cat "$dir/commit" 2>/dev/null || true)
if [[ ! $commit =~ ^[0-9a-f]{40}$ ]]; then
    echo "$dir: the measured source wasn't committed; only committed versions are uploaded" >&2
    exit 1
fi

export BENCHER_PROJECT=${BENCHER_PROJECT:-ren}
bencher run \
    --branch "${BRANCH:-master}" \
    --hash "$commit" \
    --testbed "${TESTBED:-desktop}" \
    --adapter json \
    --file "$dir/results.json"
