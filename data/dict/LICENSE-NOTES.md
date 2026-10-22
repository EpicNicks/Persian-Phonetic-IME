# Licence notes for the Persian word-frequency data

Scope: `fa-wordfreq.tsv`, `fixture.tsv`, `eval-prefixes.tsv` and any `dict.bin` built from
them. This is an engineering reading of the package text, not legal advice.

Source: PyPI `wordfreq` 3.1.1 (Robyn Speer; the project is no longer updated, data
frozen around 2021). Persian ships only as `small_fa.msgpack.gz` (31,389 entries; there is
no `large_fa`). Per the wordfreq README, Persian has 4 sources: Wikipedia, Subtitles,
Web (OSCAR), Twitter.

## Quoted terms (from wordfreq 3.1.1 `METADATA`/README "License" and `LICENSE.txt`)

Code: `LICENSE.txt`: "Copyright 2022 Robyn Speer. Licensed under the Apache License,
Version 2.0". Package metadata: `License: Apache-2.0`.

Data: "`wordfreq` is freely redistributable under the Apache license (see `LICENSE.txt`),
and it includes data files that may be redistributed under a Creative Commons
Attribution-ShareAlike 4.0 license (<https://creativecommons.org/licenses/by-sa/4.0/>)."

Sources actually used for `fa`:

- Wikipedia: listed among "data derived from the following Creative Commons-licensed
  sources: ... Wikipedia, the free encyclopedia (<http://www.wikipedia.org>)" (CC BY-SA).
- OPUS OpenSubtitles 2018: "whose data originates from the OpenSubtitles project ... and may
  be used with attribution to OpenSubtitles."
- OSCAR (web text): named in the source list ("**Web** text, from OSCAR"); the README gives
  no separate licence text for it. (Outside knowledge: OSCAR curation is CC0-style, with
  no claim over the underlying web text; wordfreq only keeps aggregate word counts.)
- Twitter: "collected by a custom application that watches the streaming Twitter API, in
  accordance with Twitter's Developer Agreement & Policy. This software gives statistics
  about words ...; it does not display or republish any Twitter content."

Not used for Persian (apply only to other languages): Google Books Ngrams, Leeds corpus,
ParaCrawl, SUBTLEX (the "credit the SUBTLEX authors" condition), NewsCrawl, Reddit.

## Conclusion

- No non-commercial (NC) or no-derivatives source for Persian. No blocker found.
- Strictest term is CC BY-SA 4.0 on the data files. The shipped list is a derived
  aggregate of word counts (facts), but treat it conservatively as a CC BY-SA 4.0
  adaptation of wordfreq's data.
- Obligations when shipping a derived binary dictionary (`dict.bin`) inside the GPL-3.0-only app:
  1. Attribution: credit wordfreq (Robyn Speer), name the Persian sources (Wikipedia,
     OpenSubtitles via OPUS, OSCAR, Twitter statistics), link CC BY-SA 4.0, and state that
     changes were made (filtered, normalised, converted). Put this in the installer/about
     text and a THIRD-PARTY-NOTICES file; include the Apache-2.0 notice for wordfreq.
  2. Share-alike: applies to the dictionary data (the adaptation), not to the GPL code,
     provided the dictionary is a separate file loaded at runtime (do not `include_bytes!`
     it into the DLL). Distribute the dictionary under CC BY-SA 4.0 and give the build
     recipe (`tools/wordlist/`), which is public. Keep the data file licence marking
     separate from the GPL code licence in README/LICENSE.
  3. No additional restrictions (no DRM-style terms on the dictionary file).
- Red flags: (a) wordfreq is unmaintained, data is dated (slang/new words missing);
  (b) Twitter/OSCAR/OpenSubtitles text is user-generated and includes profanity and noisy
  tokens, which may need a filter for suggestions; (c) the share-alike wording is
  conservative: if embedding in the DLL is ever wanted, get a proper legal read first.
