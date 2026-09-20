"""Gate the translation catalogue on what po2lmo actually does with it.

Run from the repository root:

    python3 tools/check_catalog.py              # check the committed catalogue
    python3 tools/check_catalog.py --self-test  # prove the checks can fail

Why this exists
---------------
`msgfmt --check` validates PO syntax, and `tools/sync_po.py --check` validates
that the catalogue matches the sources.  Neither looks at whether the catalogue
can translate anything, and the gap between them is not hypothetical:

* po2lmo drops every entry whose msgid and msgstr hash to the same value - there
  is nothing to look up.  A catalogue made only of such entries compiles to zero
  entries, and po2lmo then does `fclose(out); unlink(argv[2]);` and exits 0 (see
  the `offset > 0` branch at the end of po2lmo.c).  The i18n package is built
  successfully and ships **no .lmo at all**, so every _() call returns its msgid.
* A menu.d title whose msgstr equals its msgid is the same trap one level up:
  the title is an English msgid in JSON, LuCI resolves it through the catalogue,
  and an identity entry means that page keeps its English title forever.

luci-i18n-h5000m-netmode-zh-cn 0.260919.81576 hit both: it was built with all
112 entries identity, po2lmo deleted its output, and the page title rendered as
"Exit Priority" in a Chinese UI while every existing check was green.

Note on Plural-Forms: adding a `Plural-Forms:` header would make offset > 0 and
therefore keep the file alive, but it adds only a formula entry - no message is
translated.  It would hide the symptom without fixing it, so the header stays
out and this check stays strict about real entries.
"""
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_PO = ROOT / "po/zh_Hans/h5000m-netmode.po"
DEFAULT_MENU = ROOT / "root/usr/share/luci/menu.d/luci-app-h5000m-netmode.json"


def super_fast_hash(data: bytes) -> int:
    """Byte-for-byte port of sfh_hash() in po2lmo.c (== sfh() in cbi.js).

    Verified against catalogues built by the official tool: the hashes it
    produces resolve base.zh-cn.lmo's "Status" -> "状态" and
    h5000m-fancontrol.zh-cn.lmo's "Fan Control" -> "风扇控制".
    """
    length = len(data)
    if length <= 0:
        return 0
    h = length & 0xFFFFFFFF
    pos, size = 0, length >> 2
    while size:
        h = (h + int.from_bytes(data[pos:pos + 2], "little")) & 0xFFFFFFFF
        tmp = ((int.from_bytes(data[pos + 2:pos + 4], "little") << 11) ^ h) & 0xFFFFFFFF
        h = ((h << 16) ^ tmp) & 0xFFFFFFFF
        h = (h + (h >> 11)) & 0xFFFFFFFF
        pos += 4
        size -= 1
    rem = length & 3
    if rem == 3:
        h = (h + int.from_bytes(data[pos:pos + 2], "little")) & 0xFFFFFFFF
        h = (h ^ (h << 16)) & 0xFFFFFFFF
        c = data[pos + 2]
        h = (h ^ ((c if c < 128 else c - 256) << 18)) & 0xFFFFFFFF
        h = (h + (h >> 11)) & 0xFFFFFFFF
    elif rem == 2:
        h = (h + int.from_bytes(data[pos:pos + 2], "little")) & 0xFFFFFFFF
        h = (h ^ (h << 11)) & 0xFFFFFFFF
        h = (h + (h >> 17)) & 0xFFFFFFFF
    elif rem == 1:
        c = data[pos]
        h = (h + (c if c < 128 else c - 256)) & 0xFFFFFFFF
        h = (h ^ (h << 10)) & 0xFFFFFFFF
        h = (h + (h >> 1)) & 0xFFFFFFFF
    h = (h ^ (h << 3)) & 0xFFFFFFFF
    h = (h + (h >> 5)) & 0xFFFFFFFF
    h = (h ^ (h << 4)) & 0xFFFFFFFF
    h = (h + (h >> 17)) & 0xFFFFFFFF
    h = (h ^ (h << 25)) & 0xFFFFFFFF
    h = (h + (h >> 6)) & 0xFFFFFFFF
    return h


def po_unescape(s: str) -> str:
    out, i, esc = [], 0, {"n": "\n", "t": "\t", "r": "\r", "\\": "\\", '"': '"',
                          "a": "\a", "b": "\b", "f": "\f", "v": "\v", "0": "\0"}
    while i < len(s):
        if s[i] == "\\" and i + 1 < len(s):
            out.append(esc.get(s[i + 1], s[i + 1]))
            i += 2
        else:
            out.append(s[i])
            i += 1
    return "".join(out)


def parse_po(text: str):
    """[(msgid, msgstr)]; the metadata header (msgid "") is dropped."""
    out = []
    for block in re.split(r"\n\s*\n", text):
        if "msgid" not in block:
            continue

        def field(name):
            m = re.search(r'^%s((?:\s*"(?:[^"\\]|\\.)*")+)' % name, block, re.M)
            if not m:
                return None
            return po_unescape("".join(re.findall(r'"((?:[^"\\]|\\.)*)"', m.group(1))))

        mid = field("msgid")
        if not mid:
            continue
        out.append((mid, field("msgstr") or ""))
    return out


def menu_titles(text: str):
    return re.findall(r'"title"\s*:\s*"([^"]+)"', text)


def audit(entries, titles):
    """Return (problems, stats).  Pure so --self-test can drive it both ways."""
    surviving = [(m, t) for m, t in entries if super_fast_hash(m.encode()) != super_fast_hash(t.encode())]
    blob = sum(len(t.encode()) + ((4 - len(t.encode()) % 4) % 4) for _, t in surviving)

    problems = []
    if not surviving:
        problems.append(
            "no entry survives po2lmo: every msgstr equals its msgid, so the "
            "compiler writes nothing, unlinks its output and exits 0 - the i18n "
            "package would ship without any .lmo and every string would render "
            "in its source language")

    table = dict(entries)
    for title in titles:
        if title not in table:
            problems.append("menu title %r is missing from the catalogue" % title)
        elif table[title] == title:
            problems.append(
                "menu title %r has an identity translation: LuCI resolves menu "
                "titles through the catalogue, so this page would keep its "
                "English title" % title)

    return problems, {"entries": len(entries), "surviving": len(surviving),
                      "blob": blob, "titles": len(titles)}


def self_test():
    """Positive and negative control: the audit must reject the v1.6.0 catalogue
    and accept a repaired one.  A check that cannot fail is not a check."""
    titles = ["Exit Priority"]
    identity = [("网络出口", "网络出口"), ("Exit Priority", "Exit Priority")]
    fixed = [("网络出口", "网络出口"), ("Exit Priority", "出口优先级")]

    bad, _ = audit(identity, titles)
    good, _ = audit(fixed, titles)
    if not bad:
        print("self-test FAILED: an identity-only catalogue was accepted")
        return 1
    if good:
        print("self-test FAILED: a repaired catalogue was rejected: %s" % good)
        return 1
    if not any("English title" in p for p in bad):
        print("self-test FAILED: identity menu title was not reported")
        return 1
    print("self-test OK (identity catalogue rejected, repaired catalogue accepted)")
    return 0


def main():
    if "--self-test" in sys.argv:
        return self_test()

    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    po = Path(args[0]) if args else DEFAULT_PO
    menu = Path(args[1]) if len(args) > 1 else DEFAULT_MENU

    entries = parse_po(po.read_text(encoding="utf-8"))
    titles = menu_titles(menu.read_text(encoding="utf-8"))
    problems, stats = audit(entries, titles)

    print("catalogue %s" % po.relative_to(ROOT))
    print("  entries              %d   (identity entries %d)"
          % (stats["entries"], stats["entries"] - stats["surviving"]))
    print("  surviving po2lmo     %d" % stats["surviving"])
    print("  value blob           %d bytes" % stats["blob"])
    print("  menu titles checked  %d" % stats["titles"])
    if problems:
        print("\ncatalogue check FAILED")
        for p in problems:
            print("  - %s" % p)
        return 1
    print("\ncatalogue check OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
