#!/usr/bin/env python3
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
"""Generates THIRD-PARTY-NOTICES.html: the licences of the crates Ren and
its Servo helper are built from (cargo-about, both workspaces, all
features, without the crates only tests and build scripts use), the
libraries vendored inside crates (SpiderMonkey's, in mozjs_sys), and what
isn't a crate (notices/header.html).

    scripts/notices.py [--check]

--check compares with the committed file instead of writing it (CI,
with the same cargo-about version: 0.9.2).
Offline after `cargo fetch` (so the result doesn't depend on the
network): crates that lack their licence file get the licence's
standard text.
"""

import html
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "THIRD-PARTY-NOTICES.html"
WORKSPACES = [ROOT / "Cargo.toml", ROOT / "crates/ren-servo/Cargo.toml"]

# Libraries inside mozjs_sys (SpiderMonkey's source), which cargo-about
# can't see: their licence files in the crate, or in notices/.
VENDORED = [
    (
        "ICU",
        "Unicode's internationalisation library, in SpiderMonkey for "
        "Intl and Unicode data (ICU 78).",
        None,
        "notices/icu-78.txt",
    ),
    (
        "irregexp",
        "V8's regular expression engine, in SpiderMonkey.",
        "mozjs/js/src/irregexp/LICENSE.v8",
        None,
    ),
    (
        "double-conversion",
        "Conversions between numbers and text, in SpiderMonkey.",
        "mozjs/mfbt/double-conversion/LICENSE",
        None,
    ),
    (
        "fdlibm",
        "The math functions of SpiderMonkey.",
        "mozjs/modules/fdlibm/src/LICENSE",
        None,
    ),
    (
        "LZ4",
        "Compression, in SpiderMonkey's support library (mozglue).",
        "mozjs/mozglue/static/lz4/LICENSE",
        None,
    ),
    (
        "function2",
        "Function wrappers, in SpiderMonkey's support code.",
        "mozjs/third_party/function2/LICENSE.txt",
        None,
    ),
]


def cargo_about(manifest: Path) -> dict:
    """cargo-about's licences of a workspace, as JSON."""
    subprocess.run(
        ["cargo", "fetch", "--locked", "--manifest-path", str(manifest)],
        check=True,
        cwd=manifest.parent,
    )
    result = subprocess.run(
        [
            "cargo", "about", "generate", "--format", "json", "--all-features",
            "--frozen", "--manifest-path", str(manifest),
            "-c", str(ROOT / "about.toml"),
        ],
        check=True,
        capture_output=True,
        text=True,
        cwd=manifest.parent,
    )
    return json.loads(result.stdout)


def package_dir(manifest: Path, name: str) -> Path:
    """The source directory of a package of a workspace."""
    result = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--frozen",
         "--manifest-path", str(manifest)],
        check=True,
        capture_output=True,
        text=True,
    )
    for package in json.loads(result.stdout)["packages"]:
        if package["name"] == name:
            return Path(package["manifest_path"]).parent
    raise SystemExit(f"no package {name} in {manifest}")


def normalised(text: str) -> str:
    return " ".join(text.split())


def merge(reports: list) -> list:
    """The licence texts of all reports, each once, with the crates (not
    Ren's own) that use it, sorted."""
    texts = {}
    for report in reports:
        for licence in report["licenses"]:
            users = {
                (user["crate"]["name"], user["crate"]["version"])
                for user in licence["used_by"]
                # Ren's own crates come from paths, not a registry.
                if user["crate"]["source"] is not None
            }
            if not users:
                continue
            key = (licence["id"], normalised(licence["text"]))
            entry = texts.setdefault(
                key, {"name": licence["name"], "text": licence["text"], "users": set()}
            )
            entry["users"] |= users
    entries = [
        {**entry, "id": key[0], "users": sorted(entry["users"])}
        for key, entry in texts.items()
    ]
    entries.sort(key=lambda e: (e["id"], e["users"][0]))
    return entries


def pre(text: str) -> str:
    lines = [line.rstrip() for line in text.strip("\n").splitlines()]
    return "<pre>" + html.escape("\n".join(lines), quote=False) + "</pre>"


def generate() -> str:
    reports = [cargo_about(manifest) for manifest in WORKSPACES]
    mozjs = package_dir(WORKSPACES[1], "mozjs_sys")
    parts = [(ROOT / "notices/header.html").read_text().rstrip(), ""]

    parts.append("<h2>Libraries inside crates</h2>")
    parts.append(
        "<p>SpiderMonkey, Servo's JavaScript engine, comes with libraries "
        "of its own in the crate mozjs_sys (listed below under its "
        "licence, the MPL 2.0):</p>"
    )
    for title, about, in_crate, in_repo in VENDORED:
        path = mozjs / in_crate if in_crate else ROOT / in_repo
        parts.append(f"<h3>{html.escape(title)}</h3>")
        parts.append(f"<p>{html.escape(about)}</p>")
        parts.append(pre(path.read_text(errors="replace")))
    parts.append("")

    entries = merge(reports)
    crates = len({user for entry in entries for user in entry["users"]})
    parts.append("<h2>Rust crates</h2>")
    parts.append(
        f"<p>The {crates} crates Ren and its web page helper are built "
        "from, by licence text.</p>"
    )
    for entry in entries:
        users = ", ".join(f"{name} {version}" for name, version in entry["users"])
        parts.append(f"<h3>{html.escape(entry['name'])}</h3>")
        parts.append(f"<p>Used by {html.escape(users)}.</p>")
        parts.append(pre(entry["text"]))
    return "\n".join(parts) + "\n"


def main() -> None:
    text = generate()
    if sys.argv[1:] == ["--check"]:
        if OUTPUT.read_text() != text:
            raise SystemExit(
                f"{OUTPUT.name} doesn't match Cargo.lock: run `just notices`"
            )
        print(f"{OUTPUT.name} is up to date")
        return
    OUTPUT.write_text(text)
    print(f"{OUTPUT.name}: {len(text) // 1024} KiB")


if __name__ == "__main__":
    main()
