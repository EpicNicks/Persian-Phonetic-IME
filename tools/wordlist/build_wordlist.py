#!/usr/bin/env python3
"""Export a cleaned Persian word-frequency list from the `wordfreq` package.

Output: UTF-8 TSV, one `word<TAB>zipf` per line, sorted by zipf descending
(docs/autocomplete.md section 3.2). zipf = log10(occurrences per billion words),
2 decimals. Counts are summed as integers (round(freq * 1e9)) while spellings
collapse, and converted to zipf only at the end. --min-zipf filters on zipf.

Autocomplete-specific drops (counted in the summary): standalone morphemes
(MORPHEMES below) and words listed in tools/wordlist/blocklist.txt.

Normalisation (each drop is counted in the summary):
  NFC; Arabic ي ى -> ی, ك -> ک, ة -> ه, ھ -> ه; tatweel and harakat stripped
  (merged); words with Latin, digits (any script), other scripts or symbols
  dropped; ZWNJ canonicalised (see below); single letters other than و dropped;
  entries that normalise to the same word have frequencies summed.

ZWNJ: wordfreq's Persian tokeniser leaves ZWNJ inside tokens for compounds
(می‌شود) but also emits fragments with a leading or trailing ZWNJ (`می‌`,
`‌ها`) whose base word is counted separately. Those fragments are DROPPED, not
merged, to avoid double counting the base word. Repeated ZWNJ collapse to one.
A ZWNJ after a non-joining letter (ا آ د ذ ر ز ژ و ؤ أ ء) has no visible effect
and is removed (merged). Spaced/glued variants (میروم vs می‌روم) stay separate
entries: they are distinct spellings people type.
"""
import argparse
import collections
import math
import os
import re
import sys
import unicodedata

ZWNJ = "\u200c"
SCALE = 1_000_000_000
MAP = {"\u064a": "\u06cc", "\u0649": "\u06cc", "\u0643": "\u06a9", "\u0629": "\u0647",
       "\u06be": "\u0647"}
# harakat, superscript alef, hamza marks above/below, tatweel
STRIP = {chr(c) for c in range(0x064B, 0x0660)} | {"\u0670", "\u0640"}
LETTERS = set("ءآأؤإئابپتثجچحخدذرزژسشصضطظعغفقکگلمنوهی")
NON_JOINERS = set("اآأإءؤدذرزژو")
KEEP_SINGLE = {"و"}
# Bare suffixes / prefixes that wordfreq counts as tokens but that make poor
# completions (users type them after a ZWNJ, not as words). Exact match, after
# normalisation. Real words that merely look like suffixes (ای, بی, هم) are kept.
MORPHEMES = {
    "ها", "های", "هایی", "هایم", "هایت", "هایش", "هایمان", "هایتان", "هایشان",  # plural
    "می", "نمی",                                                               # verb prefix
    "تر", "ترین", "تری",                                                       # comparative
    "ام", "ات", "اش", "مان", "تان", "شان",                                     # possessive
    "ایم", "اید", "اند", "یم", "ید", "ند",                                     # verb endings
    "ین", "گان", "گی",                                                         # misc suffixes
}


def classify_chars(w):
    """Return a drop reason for the first offending char, else None."""
    for ch in w:
        if ch in LETTERS or ch == ZWNJ:
            continue
        cat = unicodedata.category(ch)
        if "a" <= ch.lower() <= "z" or unicodedata.name(ch, "").startswith("LATIN"):
            return "latin"
        if cat == "Nd" or ch.isdigit():
            return "digit"
        if cat.startswith("L"):
            return "other-script-or-letter"
        return "symbol"
    return None


def normalise(raw):
    """Return (word, None) or (None, drop_reason)."""
    w = unicodedata.normalize("NFC", raw)
    w = "".join(MAP.get(c, c) for c in w)
    w = unicodedata.normalize("NFC", w)
    w = "".join(c for c in w if c not in STRIP)
    r = classify_chars(w)
    if r:
        return None, r
    w = re.sub(ZWNJ + "+", ZWNJ, w)
    if not w.strip(ZWNJ):
        return None, "zwnj-only"
    if w[0] == ZWNJ or w[-1] == ZWNJ:
        return None, "zwnj-fragment"
    out = []
    for i, c in enumerate(w):
        if c == ZWNJ and out and out[-1] in NON_JOINERS:
            continue
        out.append(c)
    w = "".join(out)
    if re.search(r"(.)\1\1", w):
        return None, "letter-run-3plus"
    if len(w.replace(ZWNJ, "")) == 1 and w not in KEEP_SINGLE:
        return None, "single-letter"
    return w, None


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--out", required=True)
    ap.add_argument("--top", type=int, default=200000)
    ap.add_argument("--min-zipf", type=float, default=None,
                    help="drop words with log10(per-billion freq) below this (e.g. 3.0)")
    ap.add_argument("--blocklist",
                    default=os.path.join(os.path.dirname(os.path.abspath(__file__)), "blocklist.txt"))
    ap.add_argument("--wordlist", default="best", help="wordfreq list name (fa only has small)")
    a = ap.parse_args()

    import wordfreq
    src = wordfreq.get_frequency_dict("fa", a.wordlist)
    merged = collections.defaultdict(int)
    dropped = collections.Counter()
    changed = 0
    for raw, f in src.items():
        w, why = normalise(raw)
        if why:
            dropped[why] += 1
            continue
        if w != raw:
            changed += 1
        merged[w] += max(1, round(f * SCALE))
    block = set()
    with open(a.blocklist, encoding="utf-8") as fh:
        for line in fh:
            line = line.split("#", 1)[0].strip()
            if line:
                block.add(normalise(line)[0] or line)
    n_morph = sum(1 for w in merged if w in MORPHEMES)
    n_block = sum(1 for w in merged if w in block)
    items = sorted(((w, f) for w, f in merged.items() if w not in MORPHEMES and w not in block),
                   key=lambda kv: (-kv[1], kv[0]))
    if a.min_zipf is not None:
        items = [(w, f) for w, f in items if math.log10(f) >= a.min_zipf]
    items = items[: a.top]
    with open(a.out, "w", encoding="utf-8", newline="\n") as fh:
        for w, f in items:
            fh.write(f"{w}\t{math.log10(f):.2f}\n")
    from importlib.metadata import version
    print(f"wordfreq {version('wordfreq')}", file=sys.stderr)
    print(f"source entries: {len(src)}", file=sys.stderr)
    for k, v in sorted(dropped.items(), key=lambda kv: -kv[1]):
        print(f"  dropped {k}: {v}", file=sys.stderr)
    print(f"  entries rewritten by normalisation: {changed}", file=sys.stderr)
    print(f"  merged away (collisions): {len(src) - sum(dropped.values()) - len(merged)}",
          file=sys.stderr)
    print(f"  dropped morphemes: {n_morph}; blocklisted: {n_block}", file=sys.stderr)
    print(f"written: {len(items)} -> {a.out}", file=sys.stderr)


if __name__ == "__main__":
    main()
