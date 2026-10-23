# Config schema reference

Precise reference for `data/presets/*.toml` and the user file `%APPDATA%\PersianPhoneticIme\config.toml`. `PLAN.md` section 3 is the design; if they disagree, PLAN.md wins. Items marked **(addition)** are not in PLAN.md 3.3 and need WS2/user sign-off.

## 1. Resolution order

1. Start from the built-in preset named by top-level `preset` (default `"qamooscheh"`; a preset file itself has no `preset` key).
2. Merge `[options]`: user values override the preset's values (per key).
3. Expand the merged options into map patches, in this fixed order: `s_order`, `z_order`, `gh`, `q`, `c`. (So `c = "s"` copies the already-reordered `s` list.) `kh`, `ch` and the behaviour options produce no patch.
4. Apply raw `[map.singles]`, `[map.digraphs]`, `[map.trigraphs]` entries last. A raw entry replaces the whole candidate list for that key, so it wins over any option patch.
5. Validate (section 5) and freeze into `ResolvedConfig`. Presets are validated the same way (a repo lint test).

Each of steps 2-5 is applied entry by entry, so one bad entry never discards the rest (section 6).

## 2. `[options]`

| Option | Type | Allowed values | Default | Effect |
|---|---|---|---|---|
| `gh` | string | `"ghain"`, `"qaf"` | `"ghain"` | Promotes the named letter to index 0 of `digraphs.gh`, keeping the other as alternate. `"qaf"` gives `["ق","غ"]`, `"ghain"` gives `["غ","ق"]`. If the letter is absent it is inserted at index 0 (so `minimal` + `qaf` yields `["ق","غ"]`). |
| `q` | string | `"qaf"`, `"ghain"` | `"qaf"` | Same promotion on `singles.q`: `"qaf"` -> `["ق","غ"]`, `"ghain"` -> `["غ","ق"]`. |
| `c` | string | `"none"`, `"k"`, `"s"` | `"none"` | `"none"`: remove `singles.c`; a lone `c` renders as a visible Latin `c` with the pending attribute. `"k"`: `singles.c = ["ک"]`. `"s"`: `singles.c` = copy of the current `singles.s` list (default `["س","ص","ث"]`). `ch` still renders چ in all three because the whole preedit is re-tokenised (longest match). |
| `kh` | string | `"khe"` | `"khe"` | Names the sound of `kh` (خ). Single legal value; reserved. No patch. |
| `ch` | string | `"che"` | `"che"` | Names the sound of `ch` (چ). Single legal value; reserved (Hebrew-style variant dropped, D17). No patch. |
| `s_order` | list of strings | each a valid candidate (section 5) | absent | Replaces `singles.s` wholesale. Any non-empty duplicate-free list is accepted (not only permutations), e.g. `["ص","س","ث"]`. Absent = preset's `s` list. |
| `z_order` | list of strings | as `s_order` | absent | Replaces `singles.z` wholesale. |
| `digits` | string | `"persian"`, `"ascii"` | `"persian"` | `0-9` -> U+06F0..U+06F9, or ASCII. Digits are eaten and inserted by the IME either way. |
| `punctuation` | string | `"persian"`, `"none"` | `"persian"` | `persian`: `,`->U+060C `;`->U+061B `?`->U+061F `%`->U+066A. `none`: ASCII pass-through. See `data/punctuation.toml`. |
| `guillemets` **(addition)** | string | `"none"`, `"angle"`, `"brace"` | `"none"` | `angle`: `<`->U+00AB `>`->U+00BB. `brace`: `{`->U+00AB `}`->U+00BB. PLAN 2.2 says "optional «»" without naming the option. |
| `shift_letters` | string | `"lowercase"`, `"alternate"` | `"lowercase"` | `lowercase`: Shift+letter folds to lowercase. `alternate`: Shift+letter picks that token's second candidate (A -> آ, S -> ص); a token with one candidate uses its first. |
| `space` | string | `"preedit"`, `"top-candidate"` | `"preedit"` | Commit exactly what is shown, or the top candidate. In v1 (no dictionary) both are identical. |
| `enter` | string | `"commit"`, `"commit-and-pass"` | `"commit"` | `commit`: commit and eat Enter. `commit-and-pass`: commit, then pass Enter to the app. A highlighted candidate is committed first either way. |
| `number_select` | bool | `true`, `false` | `true` | While the candidate list is open, `1`-`9` picks that candidate and commits. Off (or no list, or number out of range): digits are typed per `digits`. |
| `candidate_labels` | bool | `true`, `false` | `false` | Learner hints under glyphs; UI only. |
| `candidate_position` | string | `"below"`, `"above"` | `"below"` | Where the candidate window opens: below the preedit (Google Japanese IME style) or above it. UI only; the window still flips to the other side when there is no room. |
| `toggle_hotkey` | string | `"none"`, ``"alt+`"``, ``"ctrl+`"``, `"ctrl+space"`, `"ctrl+shift+space"`, `"ctrl+alt+space"` | `"none"` | Shortcut that switches between Persian and Latin typing (registered with TSF as a preserved key; applies after a config reload). |
| `candidate_size` | integer | 25-300 (clamped) | `50` | Size of the candidate window in percent; 100 is the original large size. UI only. |
| `theme` **(addition)** | string | `"system"`, `"light"`, `"dark"` | `"system"` | Look of the settings window; `system` follows the Windows app mode. Settings app only; the installer can preset it. |
| `glass_opacity` **(addition)** | integer | 20-100 (clamped by the settings app) | `100` | How solid the settings window's glass panels are, in percent. Settings app only. |
| `allow_arabic_forms` **(addition: location)** | bool | | `false` | Silences the Arabic-only code point warning (U+064A, U+0643, U+0629). PLAN names the flag but not where it lives. |
| `allow_sequences` **(addition: location)** | bool | | `false` | Permits candidates of more than one code point (e.g. `"لا"`). |

## 3. `[autocomplete]` (phase 2; parsed and validated, otherwise ignored in v1)

| Key | Type | Default |
|---|---|---|
| `enabled` | bool | `false` |
| `predict_min_chars` | integer >= 1 | `2` |
| `learning` | bool | `true` |

## 4. Raw map sections

```toml
[map.singles]     # keys: exactly 1 printable ASCII char (letters, ')
[map.digraphs]    # keys: exactly 2 chars
[map.trigraphs]   # keys: exactly 3 chars
```

Value: non-empty array of candidate strings, index 0 = default (shown in the preedit), the rest are alternates in list order. Key length must match its section **(assumption)**. Keys are lowercase; uppercase input is folded before lookup (or handled by `shift_letters`). Tokeniser: scan left to right, at each position take the longest key (3, then 2, then 1) present in the resolved map; a character with no key (a lone `c` under `c = "none"`) is emitted as itself, flagged pending. ZWNJ (Shift+Space) is a separate token that no key can span. Digraph starters are derived, never stored. The preset `qamooscheh` has `e` identical to `a` (lint-enforced).

## 5. Validation rules

Keys: 1-3 printable ASCII (U+0021..U+007E). Candidate lists: non-empty, no duplicates. Each candidate: exactly one NFC code point in U+0600..U+06FF, U+0750..U+077F, U+FB50..U+FDFF, U+FE70..U+FEFF, or ZWNJ U+200C (longer strings only with `allow_sequences`). Arabic-only U+064A ي, U+0643 ك, U+0629 ة warn unless `allow_arabic_forms` (the map uses ی U+06CC, ک U+06A9). A key of length n >= 2 whose (n-1)-prefix is not defined (e.g. `shh` with no `sh`, `xyz` with no `xy`) warns; it is still applied and typeable. Option values must be one of the allowed values and of the right TOML type. Unknown tables and keys are errors with a suggestion when an edit-distance match exists.

## 6. Fallback and error kinds

Every load yields a `ResolvedConfig` plus a list of issues (`severity` `Error` = something was ignored, `Warning` = applied but suspicious; each has line, column, snippet, message, hint). Golden `fallback` values: `whole_file` (TOML unparseable, entire user file skipped, preset used), `entry` (at least one entry skipped, rest applied), `none` (nothing skipped; warnings allowed). Messages follow PLAN 3.4: plain language, name `config.toml` and the line, quote the offender, list allowed values, say what was skipped, never mention serde/TOML internals, never include typed content.

| Kind | Severity | Fallback | Message wording (placeholders in `{}`) |
|---|---|---|---|
| `syntax` | Error | whole_file | `config.toml, line {n}: this line couldn't be read ({reason}).` + `Your whole settings file was skipped and the default settings are in use.` + `Fix the line and save the file; the change is picked up automatically.` Reasons: `a closing quote is missing`, `a closing bracket is missing`, `an "=" is missing`, `"{key}" is set twice` (duplicate key). |
| `unknown_key` | Error | entry | `config.toml, line {n}: unknown setting "{key}". Did you mean "{suggestion}?"` (second sentence only if a close match exists). Same shape for tables: `unknown section "{name}"`. |
| `invalid_value` | Error | entry | `"{value}" isn't one of the choices for {key}. Use "{a}" or "{b}".` then `That setting was skipped; everything else in your file is still in use.` (3+ values: `Use "a", "b" or "c".`) |
| `wrong_type` | Error | entry | `{key} must be {expected} (for example {example}), not {found}.` expected: `text`, `true or false`, `a list`, `a whole number`. |
| `unknown_preset` | Error | entry | `unknown preset "{name}". Available presets: "qamooscheh", "minimal". The default preset is in use.` |
| `bad_map_key` | Error | entry | `"{key}" can't be used as a key here: keys are 1 to 3 plain ASCII characters` / `... a key in [map.digraphs] has exactly 2 characters; "{key}" belongs in [map.{section}]`. |
| `empty_candidates` | Error | entry | `{key} needs at least one candidate letter.` |
| `duplicate_candidates` | Error | entry | `{key} lists "{c}" twice.` |
| `bad_candidate` | Error | entry | `"{c}" isn't a Persian letter` (outside the allowed blocks) / `"{c}" is more than one character; each candidate must be one character (set allow_sequences = true to allow longer ones).` |
| `arabic_form` | Warning | none | `{key} uses "{c}" (U+064A), the Arabic form, not the Persian one ("ی" U+06CC). It was applied. Set allow_arabic_forms = true to hide this warning.` |
| `unreachable_prefix` | Warning | none | `{key} is applied, but its start "{prefix}" has no entry, so typing it step by step shows the letters separately first.` |

All issues in one file are reported together (one bad entry never hides another). Applied entries are never undone by later errors.

## 7. Golden `expect.applied` convention

`applied` maps an option name (`"digits"`) or a dotted map path (`"map.singles.b"`, `"autocomplete.enabled"`) to the effective resolved value after fallback. Skipped entries show the preset's value.

## 8. `[autocomplete]` (autocomplete phase; supersedes the table in section 3)

Contract and behaviour: [`autocomplete.md`](autocomplete.md). Validated through `autocomplete_specs()` like `[options]`; messages use the existing kinds of section 6.

| Key | Type | Allowed | Default | Effect |
|---|---|---|---|---|
| `enabled` | bool | `true`, `false` | `false` | Show dictionary word completions below the alternates. Off = v1 behaviour exactly. |
| `predict_min_chars` | integer | >= 1 | `2` | Persian letters (ZWNJ not counted) in the preedit before completions appear. `0` or less: `invalid_value`, default kept. |
| `learning` | bool | `true`, `false` | `true` | Remember accepted / passed-over completions locally (`%LOCALAPPDATA%\PersianPhoneticIme\history.bin`) and use them for ranking. Off: nothing recorded, history ignored, the file is left as is. |
| `max_completions` **(addition)** | integer | 1-9 (clamped) | `5` | Completion rows shown; values above 9 become 9 silently (like `candidate_size`); `0` or less: `invalid_value`, default kept. The popup never exceeds 9 rows, so fewer appear when there are many alternates. |

Example messages (wording per section 6):

```
config.toml, line 14: enabled must be true or false (for example true), not text.
config.toml, line 15: max_completions must be a whole number (for example 5), not text.
config.toml, line 3: unknown setting "max_completion". Did you mean "max_completions"?
```

Golden `expect.applied` keys: `autocomplete.enabled`, `autocomplete.predict_min_chars`, `autocomplete.learning`, `autocomplete.max_completions`.
