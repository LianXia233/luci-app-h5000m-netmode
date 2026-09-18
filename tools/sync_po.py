"""Regenerate the zh_Hans catalogue from the strings the view actually uses.

The committed .po described an older revision of the page (Exit Mode / Exit
policy / Preferred exit), none of which the current view contains, so every
translation in it was inert: the page's msgids are Chinese and the catalogue
had no entry for them.  This script extracts the msgids from the source of
truth - the view - and writes a catalogue that matches it, so the file stops
being dead weight and a future translator has an accurate base.

Plural forms are not used by this app, so each entry is a plain msgid/msgstr
pair.  Run from the repository root:

    python tools/sync_po.py
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VIEW = ROOT / "htdocs/luci-static/resources/view/h5000m/netmode.js"
MENU = ROOT / "root/usr/share/luci/menu.d/luci-app-h5000m-netmode.json"
ACL = ROOT / "root/usr/share/rpcd/acl.d/luci-app-h5000m-netmode.json"
OUT = ROOT / "po/zh_Hans/h5000m-netmode.po"

HEADER = (
    'msgid ""\n'
    'msgstr ""\n'
    '"Project-Id-Version: luci-app-h5000m-netmode\\n"\n'
    '"PO-Revision-Date: {date}\\n"\n'
    '"Last-Translator: FAN789\\n"\n'
    '"Language-Team: Chinese (Simplified)\\n"\n'
    '"Language: zh_CN\\n"\n'
    '"MIME-Version: 1.0\\n"\n'
    '"Content-Type: text/plain; charset=UTF-8\\n"\n'
    '"Content-Transfer-Encoding: 8bit\\n"\n'
)


def view_msgids(text):
    """Every literal passed to _(), in source order, de-duplicated."""
    out = []
    for m in re.finditer(r"_\(\s*'((?:[^'\\]|\\.)*)'", text):
        s = m.group(1).replace("\\'", "'").replace('\\"', '"')
        if s not in out:
            out.append(s)
    return out


def json_titles(text):
    """Menu titles are translated by LuCI too, so they belong in the catalogue."""
    return re.findall(r'"title"\s*:\s*"([^"]+)"', text)


def main():
    view = VIEW.read_text(encoding="utf-8")
    ids = view_msgids(view)

    # Menu titles are English msgids in the menu.d JSON.
    for t in json_titles(MENU.read_text(encoding="utf-8")):
        if t not in ids:
            ids.append(t)

    # The view's own English literals (badges, tech names) are msgids as well.
    for lit in ["Ethernet", "5G / LTE"]:
        if lit not in ids:
            ids.append(lit)

    date = "2026-09-18 00:00+0800"
    parts = [HEADER.format(date=date)]
    for s in ids:
        escaped = s.replace("\\", "\\\\").replace('"', '\\"')
        parts.append('msgid "%s"\nmsgstr "%s"\n' % (escaped, escaped))
    rendered = "\n".join(parts)

    if "--check" in sys.argv:
        # CI mode: the catalogue must match the source.  Compared on the entry
        # list rather than the whole file so the revision date cannot make a
        # correct catalogue look stale.
        current = OUT.read_text(encoding="utf-8") if OUT.exists() else ""
        # The first entry of a .po is the metadata header (msgid ""), which the
        # generator writes separately; drop it so it is not reported as a stale
        # translation entry that never existed.
        have = [m for m in re.findall(r'^msgid "((?:[^"\\]|\\.)*)"$', current, re.M) if m]
        want = [s.replace("\\", "\\\\").replace('"', '\\"') for s in ids]
        if have != want:
            missing = [w for w in want if w not in have]
            extra = [h for h in have if h not in want]
            print("po catalogue is out of sync with the source")
            if missing:
                print("  missing %d entr(y/ies), e.g. %s" % (len(missing), missing[:5]))
            if extra:
                print("  stale %d entr(y/ies), e.g. %s" % (len(extra), extra[:5]))
            print("  run: python tools/sync_po.py")
            return 1
        print("po catalogue is in sync (%d entries)" % len(ids))
        return 0

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(rendered, encoding="utf-8")
    print("wrote %d entries to %s" % (len(ids), OUT.relative_to(ROOT)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
