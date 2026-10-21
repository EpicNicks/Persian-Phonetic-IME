#!/usr/bin/env python3
"""Sanity checks for data/dict/fa-wordfreq.tsv (stdlib only). Exit 1 on failure."""
import re
import sys

sys.stdout.reconfigure(encoding="utf-8")

path = sys.argv[1] if len(sys.argv) > 1 else "data/dict/fa-wordfreq.tsv"
rows = [l.rstrip("\n").split("\t") for l in open(path, encoding="utf-8")]
words = [w for w, _ in rows]
freqs = [float(f) for _, f in rows]
if any(not re.fullmatch(r"\d\.\d\d", f) for _, f in rows):
    sys.exit("FAIL: zipf must be d.dd")
bad = []
if len(set(words)) != len(words):
    bad.append("duplicate words")
if freqs != sorted(freqs, reverse=True):
    bad.append("not sorted desc")
for pat, name in [(r"[\u064a\u0643\u0629\u0649]", "arabic variants"), (r"[A-Za-z]", "latin"),
                  (r"[0-9\u0660-\u0669\u06f0-\u06f9]", "digits"),
                  (r"[\u064b-\u065f\u0670\u0640]", "diacritics/tatweel"),
                  (r"^\u200c|\u200c$|\u200c\u200c", "bad zwnj")]:
    if any(re.search(pat, w) for w in words):
        bad.append(name)
print(f"{len(words)} words; min zipf {min(freqs)}; top 30: {' '.join(words[:30])}")
z = [w for w in words if "\u200c" in w]
print(f"{len(z)} with ZWNJ; top: {' '.join(z[:15])}")
mor = {"ها", "های", "می", "نمی", "تر", "ترین", "ام", "ات", "اش"}
if mor & set(words):
    bad.append("standalone morphemes")
for t in ["می\u200cروم", "کتاب\u200cها", "می\u200cشود", "ایران", "کتاب"]:
    print(f"  {t!r}: {'present' if t in words else 'MISSING'}")
if bad:
    print("FAIL:", bad)
    sys.exit(1)
print("OK")
