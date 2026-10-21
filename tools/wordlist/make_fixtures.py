#!/usr/bin/env python3
"""Derive data/dict/fixture.tsv and data/dict/eval-prefixes.tsv from fa-wordfreq.tsv.

Deterministic (no randomness). Stdlib only.
  fixture.tsv       top 300 words (len >= 2, plus و) -> `word<TAB>zipf`
  eval-prefixes.tsv `prefix<TAB>expected` for 300 words sampled evenly from ranks
                    ~10..4000 with len >= 4; prefix length cycles 2,3,4 (always shorter
                    than the word). ZWNJ words are included (prefix never ends in ZWNJ).
"""
import argparse

ZWNJ = "\u200c"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", default="data/dict/fa-wordfreq.tsv")
    ap.add_argument("--dir", default="data/dict")
    ap.add_argument("--eval-prefixes", action="store_true",
                    help="also (re)write eval-prefixes.tsv (owned by the eval stream; off by default)")
    a = ap.parse_args()
    rows = [l.rstrip("\n").split("\t") for l in open(a.src, encoding="utf-8")]
    with open(f"{a.dir}/fixture.tsv", "w", encoding="utf-8", newline="\n") as f:
        for w, c in rows[:300]:
            f.write(f"{w}\t{c}\n")
    if not a.eval_prefixes:
        return
    pool = [w for w, _ in rows[10:4000] if len(w) >= 4]
    step = len(pool) / 300
    picks = [pool[int(i * step)] for i in range(300)]
    with open(f"{a.dir}/eval-prefixes.tsv", "w", encoding="utf-8", newline="\n") as f:
        for i, w in enumerate(picks):
            n = 2 + i % 3
            p = w[:n]
            if p.endswith(ZWNJ):
                p = w[: n + 1] if len(w) > n + 1 else w[: n - 1]
            f.write(f"{p}\t{w}\n")


if __name__ == "__main__":
    main()
