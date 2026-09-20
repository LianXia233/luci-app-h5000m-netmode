"""Regenerate the zh_Hans catalogue from the strings the view actually uses.

The committed .po once described an older revision of the page (Exit Mode / Exit
policy / Preferred exit), none of which the current view contains, so every
translation in it was inert.  This script extracts the msgids from the source of
truth - the view, the menu JSON - so the file stops being dead weight.  Run from
the repository root:

    python tools/sync_po.py            # write the catalogue
    python tools/sync_po.py --check    # fail if it is behind the sources

TRANSLATIONS is not decoration, it is the fix
---------------------------------------------
An entry whose msgstr equals its msgid is dropped by po2lmo - there is nothing
to look up, so the compiler skips it.  A catalogue that is nothing but such
entries compiles to *zero* entries, and po2lmo then unlinks its own output and
exits 0 (see the offset > 0 branch at the end of po2lmo.c).  The i18n package
ends up shipping no .lmo at all and every _() call returns its msgid.

That is how this plugin shipped v1.6.0: the view's strings are already Chinese,
so msgstr == msgid was "obviously fine" for 112 entries, and the only strings
that actually needed translating were the two English menu titles - which had
identity entries too.  The result was an English page title in a Chinese UI
while `msgfmt --check` and `--check` below were both green.

So: a msgid that is user visible and not already in the target language MUST
have an entry here.  tools/check_catalog.py enforces exactly that, and also
enforces that the compiled catalogue survives po2lmo at all.
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VIEW = ROOT / "htdocs/luci-static/resources/view/h5000m/netmode.js"
MENU = ROOT / "root/usr/share/luci/menu.d/luci-app-h5000m-netmode.json"
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

# msgid -> msgstr for everything that must not fall back to its English source.
# Menu titles are English by design (they are msgids in menu.d/*.json and LuCI
# resolves them through this catalogue), so they are the entries that matter.
TRANSLATIONS = {
    "Mobile Network": "移动网络",
    "Exit Priority": "出口优先级",
    "Ethernet": "以太网",
}

# Deliberately untranslated: technology labels and fragments that LuCI's other
# catalogues also keep verbatim.  Listed so that "why is this one still English"
# has an answer in the source rather than in someone's memory.
IDENTICAL_BY_DESIGN = {
    "5G / LTE",   # radio access technology, same in every locale
    "IPv4 ",      # prefix of "IPv4 <exit>", completed at runtime
    "IPv6 ",
    "WAN ",
}


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


def collect_ids():
    view = VIEW.read_text(encoding="utf-8")
    ids = view_msgids(view)

    # Menu titles are English msgids in the menu.d JSON.
    for t in json_titles(MENU.read_text(encoding="utf-8")):
        if t not in ids:
            ids.append(t)

    return ids


def escape(s):
    return s.replace("\\", "\\\\").replace('"', '\\"')


def unescape(s):
    """Inverse of escape(), plus the C escapes a .po may carry."""
    table = {"n": "\n", "t": "\t", "r": "\r", "\\": "\\", '"': '"',
             "a": "\a", "b": "\b", "f": "\f", "v": "\v", "0": "\0"}
    out, i = [], 0
    while i < len(s):
        if s[i] == "\\" and i + 1 < len(s):
            out.append(table.get(s[i + 1], s[i + 1]))
            i += 2
        else:
            out.append(s[i])
            i += 1
    return "".join(out)


def render(ids):
    parts = [HEADER.format(date="2026-09-20 00:00+0800")]
    for s in ids:
        parts.append('msgid "%s"\nmsgstr "%s"\n' % (escape(s), escape(TRANSLATIONS.get(s, s))))
    return "\n".join(parts)


def parse_entries(text):
    """[(msgid, msgstr)], header entry (msgid "") dropped, multi-line tolerant.

    Escaped quotes are part of the value: the view has strings like
    建议点击\\"对齐出口\\"。, and a naive "[^"]*" match truncates them at the first
    backslash, which makes --check compare two different strings and report a
    difference that is not there.
    """
    out = []
    for block in re.split(r"\n\s*\n", text):
        if "msgid" not in block:
            continue

        def field(name):
            m = re.search(r'^%s((?:\s*"(?:[^"\\]|\\.)*")+)' % name, block, re.M)
            if not m:
                return None
            return unescape("".join(re.findall(r'"((?:[^"\\]|\\.)*)"', m.group(1))))

        mid = field("msgid")
        if not mid:
            continue
        out.append((mid, field("msgstr") or ""))
    return out


def main():
    ids = collect_ids()
    wanted = [(s, TRANSLATIONS.get(s, s)) for s in ids]

    if "--check" in sys.argv:
        # Compared on the entry list rather than the whole file so the revision
        # date cannot make a correct catalogue look stale.  msgstr is part of the
        # comparison on purpose: a catalogue that is in sync with the sources but
        # has lost its translations is exactly the state that broke v1.6.0.
        current = OUT.read_text(encoding="utf-8") if OUT.exists() else ""
        have = parse_entries(current)
        if have != wanted:
            missing = [w for w in wanted if w not in have]
            extra = [h for h in have if h not in wanted]
            untranslated = [m for m, t in wanted if m not in TRANSLATIONS
                            and m not in IDENTICAL_BY_DESIGN and m != t]
            print("po catalogue is out of sync with the source")
            if missing:
                print("  missing %d entr(y/ies), e.g. %s" % (len(missing), missing[:5]))
            if extra:
                print("  stale %d entr(y/ies), e.g. %s" % (len(extra), extra[:5]))
            if untranslated:
                print("  %d msgid(s) have no translation: %s" % (len(untranslated), untranslated[:5]))
            print("  run: python tools/sync_po.py")
            return 1
        translated = [m for m, t in wanted if m != t]
        print("po catalogue is in sync (%d entries, %d translated)" % (len(wanted), len(translated)))
        return 0

    OUT.parent.mkdir(parents=True, exist_ok=True)
    # newline="\n" is load-bearing: without it Path.write_text() translates to the
    # platform default, so running this on Windows rewrites every line as CRLF.
    # The .po is a source file under .gitattributes eol=lf; a CRLF copy makes the
    # diff unreadable and depends on who ran the generator.
    with open(OUT, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(render(ids))

    print("wrote %d entries to %s (%d translated)"
          % (len(ids), OUT.relative_to(ROOT), len(TRANSLATIONS)))

    # A menu title without a translation is the v1.6.0 bug: it renders in English
    # no matter what the rest of the catalogue says.  tools/check_catalog.py fails
    # the build for this; here we only refuse to write it silently.
    for t in json_titles(MENU.read_text(encoding="utf-8")):
        if t not in TRANSLATIONS:
            print("WARNING: menu title %r has no translation and will render in English" % t)
    return 0


if __name__ == "__main__":
    sys.exit(main())
