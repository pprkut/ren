#!/usr/bin/env python3
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
"""Surveys the HTML of the article bodies in a `ren --dump-items` directory:
which elements, attributes and inline CSS properties they use, and how
often things that matter for the article view occur (images, lazy-loaded
images, iframes, video, tables, ...).

Usage: scripts/survey-articles.py DUMP_DIR

Prints Markdown with counts only: no article content, titles, URLs or
host names, so the output can go into a decision record.
"""

import collections
import html.parser
import json
import pathlib
import sys

TOP = 40


class Survey(html.parser.HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.elements = collections.Counter()
        self.attributes = collections.Counter()
        self.properties = collections.Counter()
        self.features = collections.Counter()
        self.article_features = set()

    def feature(self, name):
        self.features[name] += 1
        self.article_features.add(name)

    def handle_starttag(self, tag, attrs):
        self.elements[tag] += 1
        attrs = dict(attrs)
        for name in attrs:
            if name.startswith("data-"):
                self.attributes["data-*"] += 1
            elif name.startswith("aria-"):
                self.attributes["aria-*"] += 1
            else:
                self.attributes[name] += 1
        style = attrs.get("style") or ""
        for declaration in style.split(";"):
            if ":" in declaration:
                prop = declaration.split(":", 1)[0].strip().lower()
                if prop:
                    self.properties[prop] += 1
                    if prop == "float":
                        self.feature("float (inline style)")
                    if prop.startswith("display") and "grid" in declaration:
                        self.feature("display: grid")
                    if prop.startswith("display") and "flex" in declaration:
                        self.feature("display: flex")
        if attrs.get("align") in ("left", "right") and tag in ("img", "figure", "table", "div"):
            self.feature("float (align attribute)")

        if tag == "img":
            src = attrs.get("src") or ""
            self.feature("img")
            if not src or src.startswith("data:image/gif") or src.startswith("data:image/svg"):
                if any(k in attrs for k in ("data-src", "data-lazy-src", "data-original")):
                    self.feature("img: lazy (data-src, no usable src)")
                elif not src:
                    self.feature("img: no src")
            if src.startswith("data:"):
                self.feature("img: data: URL")
            elif src.startswith("http:"):
                self.feature("img: http (not https)")
            if "srcset" in attrs:
                self.feature("img: srcset")
            if "width" in attrs or "height" in attrs:
                self.feature("img: width/height attributes")
            if src.lower().split("?")[0].endswith(".svg"):
                self.feature("img: SVG file")
            if src.lower().split("?")[0].endswith(".webp"):
                self.feature("img: WebP")
            if src.lower().split("?")[0].endswith(".avif"):
                self.feature("img: AVIF")
        elif tag in ("iframe", "video", "audio", "svg", "math", "picture", "object", "embed",
                     "table", "pre", "blockquote", "figure", "details", "form", "input",
                     "button", "canvas", "font", "center", "style", "script", "link",
                     "sup", "sub", "dl", "abbr", "mark", "s", "del", "ins", "kbd"):
            self.feature(tag)


def main():
    if len(sys.argv) != 2:
        print(__doc__.strip(), file=sys.stderr)
        return 1
    directory = pathlib.Path(sys.argv[1])
    files = sorted(directory.glob("unread-*.json")) + sorted(directory.glob("starred-*.json"))
    seen = set()
    survey = Survey()
    articles = 0
    with_feature = collections.Counter()
    sizes = []
    empty = 0
    for path in files:
        for item in json.loads(path.read_text())["items"]:
            if item["id"] in seen:
                continue
            seen.add(item["id"])
            body = item.get("body") or ""
            articles += 1
            sizes.append(len(body.encode()))
            if not body.strip():
                empty += 1
            survey.article_features = set()
            survey.feed(body)
            survey.close()
            survey.reset()
            for feature in survey.article_features:
                with_feature[feature] += 1

    sizes.sort()

    def pct(p):
        return sizes[min(len(sizes) - 1, int(len(sizes) * p))] if sizes else 0

    print(f"## Article HTML survey: {articles} articles")
    print()
    print(f"- body size (bytes): median {pct(0.5)}, p90 {pct(0.9)}, p99 {pct(0.99)}, "
          f"max {sizes[-1] if sizes else 0}; empty bodies: {empty}")
    print()
    print("### Features (articles containing it / occurrences)")
    print()
    print("| feature | articles | % | occurrences |")
    print("|---|---|---|---|")
    for feature, count in with_feature.most_common():
        print(f"| {feature} | {count} | {100 * count / max(articles, 1):.1f} "
              f"| {survey.features[feature]} |")
    for title, counter in (("Elements", survey.elements), ("Attributes", survey.attributes),
                           ("Inline CSS properties", survey.properties)):
        print()
        print(f"### {title} (top {TOP})")
        print()
        print(", ".join(f"`{name}` {count}" for name, count in counter.most_common(TOP)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
