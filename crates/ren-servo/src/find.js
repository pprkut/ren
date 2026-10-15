// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

// Finds text in the page for Ren's find bar (find.rs): selects match
// `index` (round the ends) and scrolls it into view; an empty text takes
// the selection away. Returns [the match's index, the number of matches].
// Text that isn't shown (scripts, styles, hidden elements) isn't searched,
// and matches are within one text node, as Blitz's are within a block.
(function (needle, matchCase, index) {
    const selection = window.getSelection();
    if (needle === "") {
        selection.removeAllRanges();
        return [0, 0];
    }
    // A regular expression of the text, so that the case is ignored
    // without changing the text's length.
    const special = /[\\^$.*+?()[\]{}|]/g;
    const pattern = new RegExp(needle.replace(special, "\\$&"), matchCase ? "gu" : "giu");
    const root = document.body || document.documentElement;
    const matches = [];
    if (root) {
        const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
        for (let node = walker.nextNode(); node; node = walker.nextNode()) {
            const parent = node.parentElement;
            if (!parent || parent.closest("script, style, noscript, template")) {
                continue;
            }
            pattern.lastIndex = 0;
            let first = true;
            for (let found = pattern.exec(node.data); found; found = pattern.exec(node.data)) {
                // Hidden elements have no boxes; asked once per node with a
                // match.
                if (first && parent.getClientRects().length === 0) {
                    break;
                }
                first = false;
                matches.push([node, found.index, found[0].length]);
            }
        }
    }
    if (matches.length === 0) {
        selection.removeAllRanges();
        return [0, 0];
    }
    const current = ((index % matches.length) + matches.length) % matches.length;
    const [node, start, length] = matches[current];
    const range = document.createRange();
    range.setStart(node, start);
    range.setEnd(node, start + length);
    selection.removeAllRanges();
    selection.addRange(range);
    // Into view, a third from the top, unless it's in view already.
    const rect = range.getBoundingClientRect();
    const dy = rect.top < 0 || rect.bottom > window.innerHeight ? rect.top - window.innerHeight / 3 : 0;
    const dx = rect.left < 0 || rect.right > window.innerWidth ? rect.left - window.innerWidth / 3 : 0;
    if (dx !== 0 || dy !== 0) {
        window.scrollBy(dx, dy);
    }
    return [current, matches.length];
})
