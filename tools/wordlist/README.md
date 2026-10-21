# Persian word list pipeline

Produces `data/dict/fa-wordfreq.tsv` (`word<TAB>zipf[<TAB>p]`, zipf = log10 of occurrences per billion words,
2 decimals, sorted descending) from the Python `wordfreq` package (decision D15).

## Regenerate

Never install into the global Python; use a venv anywhere outside the repo.

```
python -m venv .venv-wordlist
.venv-wordlist/Scripts/python -m pip install wordfreq==3.1.1
.venv-wordlist/Scripts/python -I tools/wordlist/build_wordlist.py --out data/dict/fa-wordfreq.tsv
.venv-wordlist/Scripts/python -I tools/wordlist/make_fixtures.py  # add --eval-prefixes to rewrite eval-prefixes.tsv (eval stream owns it)
.venv-wordlist/Scripts/python -I tools/wordlist/check_wordlist.py
```

Options: `--top N` (default 200000; the Persian source only has about 31k entries),
`--min-zipf` (log10 of per-billion freq; 3 is roughly 1 per million).

Pinned: wordfreq 3.1.1 (deps resolved at the time: ftfy 6.3.1, langcodes 3.5.1,
msgpack 1.2.3, regex 2026.9.29, locate 1.1.1, wcwidth 0.9.3). Only the `small_fa` list exists.

## Normalisation (D23, D22)

NFC; ي ى -> ی, ك -> ک, ة -> ه; tatweel and harakat stripped and frequencies merged; words
with Latin letters, any digits, other scripts or symbols dropped; runs of 3+ identical
letters dropped; single letters other than و dropped. ZWNJ: repeats collapsed, tokens that
start or end with ZWNJ (tokeniser fragments like `می‌`) dropped rather than merged (the base
word is already counted), ZWNJ after a non-joining letter removed. Spaced and glued variants
(میروم / می‌روم) remain separate entries. Bare morphemes (`ها`, `های`, `می`, `تر`, `ام`, ... see `MORPHEMES` in the script) are dropped.

## Profanity flag

Words listed in `blocklist.txt` (one normalised word per line, `#` comments; the user may
extend it) are NOT dropped. They are emitted with a third tab-separated column `p`; all other
rows keep two columns. `xtask build-dict` passes the column through to `dict.bin` (format v2,
PRIOR bit 7), and the runtime setting `[autocomplete] filter_profanity` (default true, "Hide
vulgar words" in Settings) hides flagged words from suggestions; turning it off makes them
matchable again. Only unambiguous vulgarity plus words the user chose to flag are listed;
everyday ambiguous words (تخم, ناموس, بکن/بکنم/...) are not. `fixture.tsv` never contains
flagged words; `extra.tsv` has no flags.

## Outputs

- `fa-wordfreq.tsv`: 27,870 words, 46 of them flagged `p`.
- `fixture.tsv`: top 300, for unit tests.
- `extra.tsv`: hand-curated ZWNJ forms missing from wordfreq (not generated).
- `eval-prefixes.tsv`: 300 `prefix<TAB>expected` pairs (prefix length 2-4) from ranks 10-4000.

## Licence

Code Apache-2.0; Persian data CC BY-SA 4.0 (Wikipedia, OpenSubtitles, OSCAR, Twitter
statistics). See `data/dict/LICENSE-NOTES.md` for quotes and the obligations when shipping.
