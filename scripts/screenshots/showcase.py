#!/usr/bin/env python3
# SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
# SPDX-License-Identifier: GPL-3.0-or-later
"""Writes the made-up account the screenshots show (scripts/screenshots.sh):
a `--dump-items` directory and the feeds' icons for the favicon cache.

Usage: scripts/screenshots/showcase.py DUMP_DIR FAVICON_DIR BASE_URL
  BASE_URL is where scripts/screenshots/site is served, e.g.
  http://127.0.0.1:8737; the articles' images and pages come from there.

The feeds, people and texts are made up, so the screenshots show neither
real feed data nor anyone's writing but ours. The icons are drawn with
ImageMagick (`magick`).
"""

import json
import subprocess
import sys
from pathlib import Path

FOLDERS = ["Software", "Science", "Kitchen", "Comics"]

# Title, folder (index into FOLDERS, or None), icon colour.
FEEDS = [
    ("The Compiler Diaries", 0, "#e66100"),
    ("Quiet Desktop", 0, "#1c71d8"),
    ("Packet & Pipe", 0, "#2190a4"),
    ("Open Hardware Weekly", 0, "#26a269"),
    ("Field Notes", 1, "#3a7d44"),
    ("Orbital", 1, "#613583"),
    ("The Tidal Record", 1, "#1a5fb4"),
    ("Slow Bread", 2, "#865e3c"),
    ("Twelve Spices", 2, "#c01c28"),
    ("Sidewalk Sketches", 3, "#9141ac"),
    ("City Cycling Club", None, "#e5a50a"),
    ("Old Tin Radio", None, "#5e5c64"),
]

BROKEN_FEED = "Old Tin Radio"

# 2026-11-27 09:40 UTC: the newest article.
NEWEST = 1_795_772_400

FEATURED_BODY = """\
<p>A year ago we sank a thermometer on a string into the lotus pond behind
the field station and promised ourselves to read it every week. We missed
three weeks in February, when the pond was more ice than water, and one in
July, when all of us were away at once. The rest is below.</p>
<figure>
<img src="{base}/pond-chart.png" alt="Water temperature by month" width="640" height="320">
<figcaption>Water temperature at 30 cm, the mean of each month.</figcaption>
</figure>
<h2>When the leaves rise</h2>
<p>The first leaves broke the surface in the second week of May, when the
water had stayed above 15 °C for ten days in a row. They floated flat at
first; the tall leaves, held high above the water on their stalks, came a
month later. By the end of June the eastern half of the pond was covered,
and the moorhens had moved to the western half.</p>
<blockquote><p>The flowers open in the morning and close again in the
afternoon, for three days. On the third day they don't close any more,
and the petals fall.</p></blockquote>
<p>We counted 41 flowers over the summer, against 26 the year before.
Whether that is the warm June or the mud we cleared in March, a single year
can't tell. What we will keep doing:</p>
<ul>
<li>reading the thermometer every Saturday morning, ice or not;</li>
<li>counting flowers and seed heads every week from June;</li>
<li>marking the edge of the leaves on the map at the start of each month.</li>
</ul>
<p>The readings are in the station's shared folder, one row per week. Next
spring we'll add a second thermometer at the bottom of the pond, where the
rhizomes overwinter.</p>
"""

# Feed, title, author, starred, unread, a paragraph. Newest first; the
# first one is the article the screenshots open.
ITEMS = [
    ("Field Notes", "A year at the lotus pond", "Marta Visser", True, True, None),
    ("The Compiler Diaries", "Shaving two seconds off incremental builds", "Irena Kowalski", False, True,
     "Most of an incremental build was spent checking things that hadn't changed. Here is how we found out, and the three changes that got the time down from five seconds to three."),
    ("Slow Bread", "Rye, twelve hours, no kneading", "Tomás Herrera", True, True,
     "A dark rye that asks for nothing but time: mix it in the evening, shape it in the morning, and bake it before lunch."),
    ("Quiet Desktop", "Fractional scaling without the blur", "Lena Brandt", False, True,
     "Why text looked soft at 125 %, what the compositor and the toolkit each have to do about it, and where things stand now."),
    ("Orbital", "The comet that came back early", "Ade Okafor", False, True,
     "Predicted for March, it turned up in a survey image three weeks ahead of time. Outgassing, it seems, gave it a push."),
    ("Packet & Pipe", "What a home router does all day", "Sam Novak", False, True,
     "We logged a week of traffic on an ordinary home network. Most of it was devices talking to each other, and the rest wasn't what we expected either."),
    ("City Cycling Club", "Winter route: along the river to the old mill", "Joris de Vries", False, True,
     "Forty kilometres, flat, and sheltered from the wind for most of the way. Coffee at the mill at half time."),
    ("Twelve Spices", "Cardamom, green and black", "Priya Raman", False, True,
     "Two spices that share a name and little else: one bright and floral, the other smoky. When to use which, and why not to grind them in advance."),
    ("Open Hardware Weekly", "A keyboard you can repair with a screwdriver", "Mika Laine", False, True,
     "Every part is held by screws, every switch can be swapped, and the schematics are online. We took one apart to see how far that goes."),
    ("The Tidal Record", "Counting seals from the dike", "Femke Bakker", False, True,
     "Twice a year volunteers walk the dike with binoculars and tally sheets. This autumn's count was the highest in a decade."),
    ("Sidewalk Sketches", "Strip 214: The umbrella", "Noor Haddad", False, True,
     "In which the wind has opinions."),
    ("The Compiler Diaries", "Reading a flame graph, slowly", "Irena Kowalski", False, True,
     "Wide is expensive, tall is just deep. A walk through one real profile, from the widest bar down to the line that mattered."),
    ("Field Notes", "Dragonflies of the reed beds", "Marta Visser", False, True,
     "Eleven species this summer, two of them new to the station's list. A field guide to the ones you're most likely to see."),
    ("Quiet Desktop", "Dark mode that follows the sun", "Lena Brandt", False, True,
     "Switching colour schemes at sunset sounds easy until every application has to notice. How the setting travels from the desktop to the toolkits."),
    ("Slow Bread", "Why your starter smells of nail polish", "Tomás Herrera", False, True,
     "It's hungry, not dead. What the smell means, and how to bring a neglected starter back in three feedings."),
    ("Orbital", "A clear night for the Geminids", "Ade Okafor", False, True,
     "The best meteor shower of the year peaks on the night of 13 December, with a thin moon. Where to look, and how long to let your eyes adjust."),
    ("Packet & Pipe", "DNS, explained with postcards", "Sam Novak", False, False,
     "A lookup is a chain of questions, each answered with a pointer to someone who knows more. We drew it as postcards."),
    ("Old Tin Radio", "Restoring a 1950s valve set, part 3", "Walter Grün", False, True,
     "The capacitors are replaced and the dial lamp glows. Next up: the hum, and a speaker cone with a tear in it."),
    ("City Cycling Club", "Lights, mudguards, and other winter wisdom", "Joris de Vries", False, False,
     "What the club's members ride with from November to March, and what they wish they had bought sooner."),
    ("Open Hardware Weekly", "Solar logger, version two", "Mika Laine", True, False,
     "The first one drowned. The second one has a proper enclosure, a smaller battery, and has run on a balcony since June."),
    ("Twelve Spices", "A pantry tour in twelve jars", "Priya Raman", False, False,
     "Which spices earn their place on a small shelf, and which ones you can borrow from a neighbour twice a year."),
    ("The Tidal Record", "Spring tides and the moon", "Femke Bakker", False, False,
     "Why the highest tides come twice a month, and why the ones in March and September are higher still."),
    ("Sidewalk Sketches", "Strip 213: Monday", "Noor Haddad", False, False,
     "The coffee machine is also having a Monday."),
    ("The Compiler Diaries", "Error messages are user interface", "Irena Kowalski", False, False,
     "An error message is read by someone who is already having a bad time. Notes from rewriting forty of them."),
    ("Field Notes", "Mapping the pond with a rowing boat and a stick", "Marta Visser", False, False,
     "Depth soundings, the old-fashioned way: a marked pole, a rope across the pond, and an afternoon of calm weather."),
    ("Quiet Desktop", "Notifications that wait their turn", "Lena Brandt", False, False,
     "Do not disturb is a start. What it would take for notifications to know when you're presenting, typing, or simply reading."),
    ("Slow Bread", "Baking in a cold kitchen", "Tomás Herrera", False, False,
     "Dough rises slower at 17 °C, and that's not a problem. How to plan around a cold kitchen instead of fighting it."),
    ("Orbital", "Why the Moon looks bigger near the horizon", "Ade Okafor", False, False,
     "It doesn't, as a photo shows. The illusion is in our heads, and nobody quite agrees on how it works."),
    ("Packet & Pipe", "IPv6 at home, a year later", "Sam Novak", False, False,
     "We turned it on and waited for something to break. Two things did, neither of them the router."),
    ("City Cycling Club", "Results of the autumn ride", "Joris de Vries", True, False,
     "Sixty-two riders, one puncture, and a headwind on the way back that nobody will forget soon."),
    ("Open Hardware Weekly", "Soldering for beginners: the first ten joints", "Mika Laine", False, False,
     "Clean tip, hot iron, and less solder than you think. A kit that teaches it in an evening."),
    ("Twelve Spices", "Toasting seeds without burning them", "Priya Raman", False, False,
     "A dry pan, medium heat, and your nose. The moment to take them off is earlier than you think."),
]

PAGE = "pond.html"


def fnv1a(text):
    """The favicon cache's file names (crates/ren/src/hash.rs)."""
    value = 0xCBF29CE484222325
    for byte in text.encode():
        value = ((value ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return value


def main():
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    dump, favicons, base = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3].rstrip("/")
    dump.mkdir(parents=True, exist_ok=True)
    favicons.mkdir(parents=True, exist_ok=True)

    folders = [{"id": i, "name": name, "opened": True, "feeds": []} for i, name in enumerate(FOLDERS, 1)]
    feeds = []
    feed_ids = {}
    for feed_id, (title, folder, colour) in enumerate(FEEDS, 1):
        feed_ids[title] = feed_id
        icon = f"{base}/icons/{feed_id}.png"
        broken = title == BROKEN_FEED
        feeds.append({
            "id": feed_id,
            "url": f"{base}/feeds/{feed_id}.xml",
            "title": title,
            "faviconLink": icon,
            "added": NEWEST - 365 * 86400,
            "folderId": None if folder is None else folder + 1,
            "unreadCount": 0,
            "ordering": 0,
            "link": f"{base}/",
            "pinned": False,
            "updateErrorCount": 12 if broken else 0,
            "lastUpdateError": "Could not find a valid feed at the given URL: 404 Not Found" if broken else None,
            "items": [],
            "nextUpdateTime": None,
        })
        # A rounded square in the feed's colour with its initial, as
        # favicons tend to be.
        subprocess.run(
            ["magick", "-size", "32x32", "xc:none", "-fill", colour,
             "-draw", "roundrectangle 0,0 31,31 7,7", "-fill", "white",
             "-font", "DejaVu-Sans-Bold", "-pointsize", "20", "-gravity", "center",
             "-annotate", "+0+1", title.removeprefix("The ")[0],
             str(favicons / f"{fnv1a(icon):016x}.png")],
            check=True,
        )

    items = []
    count = len(ITEMS)
    when = NEWEST
    for index, (feed, title, author, starred, unread, text) in enumerate(ITEMS):
        item_id = 5000 + count - index
        body = FEATURED_BODY.format(base=base) if text is None else f"<p>{text}</p>"
        url = f"{base}/{PAGE}" if text is None else f"{base}/articles/{item_id}.html"
        items.append({
            "id": item_id,
            "guid": url,
            "guidHash": f"{fnv1a(url):032x}",
            "url": url,
            "title": title,
            "author": author,
            "pubDate": when,
            "updatedDate": None,
            "body": body,
            "enclosureMime": None,
            "enclosureLink": None,
            "mediaThumbnail": None,
            "mediaDescription": None,
            "feedId": feed_ids[feed],
            "unread": unread,
            "starred": starred,
            "filtered": False,
            "lastModified": when,
            "rtl": False,
            "fingerprint": None,
            "contentHash": None,
        })
        # Between 40 minutes and four hours apart.
        when -= 2400 + (index * 7919) % 12000

    def write(name, value):
        (dump / name).write_text(json.dumps(value, ensure_ascii=False, indent=1) + "\n")

    write("folders.json", {"folders": folders})
    write("feeds.json", {"feeds": feeds})
    write("unread-0001.json", {"items": items})


if __name__ == "__main__":
    main()
