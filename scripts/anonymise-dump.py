#!/usr/bin/env python3
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
"""Turns a `ren --dump-items` directory into anonymised test fixtures for
the nextcloud-news crate.

Usage: scripts/anonymise-dump.py DUMP_DIR [OUT_DIR]
  OUT_DIR defaults to crates/nextcloud-news/tests/fixtures/recorded.

Keeps what the decoder cares about: the keys and their order, which values
are null, numbers and booleans (ids, timestamps, counts, read and starred
state), the shape of strings (whitespace, line breaks, punctuation, length)
and of HTML bodies (elements and attribute names), and the server's JSON
escaping. Replaces all text, names, URLs, GUIDs and hashes. Equal values
stay equal (an item's guid and url, the same fingerprint in two feeds).

Takes the version, status, all folders and feeds, a sample of unread items
(the newest ones plus the first item with each rarer feature: right to
left, enclosure, thumbnail, no author, no body, filtered), a few starred
items and a few of `/items/updated`. Review the output before committing.
"""

import hashlib
import html
import html.parser
import json
import pathlib
import re
import sys

NEWEST_ITEMS = 20
STARRED_ITEMS = 5
UPDATED_ITEMS = 10

FEATURES = {
    "rtl": lambda item: item.get("rtl"),
    "enclosure": lambda item: item.get("enclosureLink"),
    "thumbnail": lambda item: item.get("mediaThumbnail"),
    "no author": lambda item: item.get("author") is None,
    "empty author": lambda item: item.get("author") == "",
    "no body": lambda item: item.get("body") is None,
    "multi-line author": lambda item: "\n" in (item.get("author") or ""),
    "filtered": lambda item: item.get("filtered"),
}


class Anonymiser:
    def __init__(self):
        self.mapped = {}

    def placeholder(self, kind, value, make):
        """The same placeholder for the same value of a kind."""
        key = (kind, value)
        if key not in self.mapped:
            count = sum(1 for k in self.mapped if k[0] == kind) + 1
            self.mapped[key] = make(count)
        return self.mapped[key]

    def url(self, value):
        if value is None:
            return None
        return self.placeholder("url", value, lambda n: f"https://site{n}.example.org/page{n}")

    def hash(self, kind, value):
        if value is None:
            return None
        return self.placeholder(
            kind, value, lambda n: hashlib.md5(f"{kind}-{n}".encode()).hexdigest()
        )

    @staticmethod
    def text(value):
        """Same length, whitespace and ASCII punctuation; letters and digits
        replaced."""
        if not isinstance(value, str):
            return value

        def mask(c):
            if c.isspace() or (c.isascii() and not c.isalnum()):
                return c
            if c.isdigit():
                return "0"
            if c.isascii():
                return "X" if c.isupper() else "x"
            return "é" if c.isalpha() else "•"

        return "".join(mask(c) for c in value)

    def body(self, value):
        if value is None:
            return None
        rebuilt = BodyRebuilder(self)
        rebuilt.feed(value)
        rebuilt.close()
        return "".join(rebuilt.out)

    def generic(self, value):
        """Unknown fields: text masked, structure kept."""
        if isinstance(value, str):
            return self.text(value)
        if isinstance(value, list):
            return [self.generic(v) for v in value]
        if isinstance(value, dict):
            return {k: self.generic(v) for k, v in value.items()}
        return value

    def folder(self, folder):
        return {
            k: self.text(v) if k == "name" else self.generic(v) if k != "id" else v
            for k, v in folder.items()
        }

    def feed(self, feed):
        out = {}
        for k, v in feed.items():
            if k in ("url", "faviconLink", "link"):
                out[k] = self.url(v)
            elif k in ("title", "lastUpdateError"):
                out[k] = self.text(v)
            elif isinstance(v, str) or isinstance(v, (list, dict)):
                out[k] = self.generic(v)
            else:
                out[k] = v
        return out

    def item(self, item):
        out = {}
        for k, v in item.items():
            if k in ("guid", "url", "enclosureLink", "mediaThumbnail"):
                out[k] = self.url(v)
            elif k == "guidHash":
                guid = self.url(item.get("guid"))
                out[k] = hashlib.md5(guid.encode()).hexdigest() if guid else self.hash(k, v)
            elif k in ("fingerprint", "contentHash"):
                out[k] = self.hash(k, v)
            elif k == "body":
                out[k] = self.body(v)
            elif k in ("enclosureMime",):
                out[k] = v
            elif isinstance(v, (str, list, dict)):
                out[k] = self.generic(v)
            else:
                out[k] = v
        return out


class BodyRebuilder(html.parser.HTMLParser):
    """HTML with the same elements and attribute names, text and attribute
    values replaced."""

    def __init__(self, anonymiser):
        super().__init__(convert_charrefs=True)
        self.anon = anonymiser
        self.out = []

    def attrs(self, attrs):
        parts = []
        for name, value in attrs:
            if value is None:
                parts.append(f" {name}")
                continue
            if name == "dir" and value.lower() in ("ltr", "rtl", "auto"):
                pass
            elif re.match(r"^(https?:)?//", value):
                value = self.anon.url(value)
            else:
                value = self.anon.text(value)
            parts.append(f' {name}="{html.escape(value)}"')
        return "".join(parts)

    def handle_starttag(self, tag, attrs):
        self.out.append(f"<{tag}{self.attrs(attrs)}>")

    def handle_startendtag(self, tag, attrs):
        self.out.append(f"<{tag}{self.attrs(attrs)} />")

    def handle_endtag(self, tag):
        self.out.append(f"</{tag}>")

    def handle_data(self, data):
        self.out.append(html.escape(self.anon.text(data), quote=False))


def php_json(value):
    """json_encode() as Nextcloud's JSONResponse does it (JSON_HEX_TAG)."""
    text = json.dumps(value, ensure_ascii=True, separators=(",", ":"))
    return text.replace("/", "\\/").replace("<", "\\u003C").replace(">", "\\u003E")


def load(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def main():
    if len(sys.argv) not in (2, 3):
        sys.exit(__doc__)
    dump = pathlib.Path(sys.argv[1])
    root = pathlib.Path(__file__).resolve().parent.parent
    out = pathlib.Path(sys.argv[2]) if len(sys.argv) == 3 else (
        root / "crates/nextcloud-news/tests/fixtures/recorded"
    )
    out.mkdir(parents=True, exist_ok=True)
    anon = Anonymiser()

    def write(name, value):
        (out / name).write_text(php_json(value), encoding="utf-8")

    version = load(dump / "version.json")
    write("version.json", {"version": version["version"]})
    write("status.json", load(dump / "status.json"))
    folders = load(dump / "folders.json")
    write("folders.json", {**folders, "folders": [anon.folder(f) for f in folders["folders"]]})
    feeds = load(dump / "feeds.json")
    feed_ids = {feed["id"] for feed in feeds["feeds"]}
    write("feeds.json", {**feeds, "feeds": [anon.feed(f) for f in feeds["feeds"]]})

    def known(items):
        return [item for item in items if item.get("feedId") in feed_ids]

    picked = {}
    missing = dict(FEATURES)
    for page in sorted(dump.glob("unread-*.json")):
        for item in known(load(page)["items"]):
            if len(picked) < NEWEST_ITEMS:
                picked[item["id"]] = item
            for name, test in list(missing.items()):
                if test(item):
                    picked[item["id"]] = item
                    del missing[name]
        if len(picked) >= NEWEST_ITEMS and not missing:
            break
    items = sorted(picked.values(), key=lambda item: item["id"], reverse=True)
    write("items.json", {"items": [anon.item(item) for item in items]})

    starred = []
    for page in sorted(dump.glob("starred-*.json")):
        starred += known(load(page)["items"])
    write("starred.json", {"items": [anon.item(i) for i in starred[:STARRED_ITEMS]]})

    updated = dump / "updated-1d.json"
    if updated.exists():
        items = known(load(updated)["items"])[:UPDATED_ITEMS]
        write("updated.json", {"items": [anon.item(item) for item in items]})

    print(f"fixtures written to {out}", file=sys.stderr)
    print(f"features without an example: {', '.join(missing) or 'none'}", file=sys.stderr)


if __name__ == "__main__":
    main()
