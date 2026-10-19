"""Lint data/: presets, punctuation, golden files. Run: python data/validate.py (exit 1 on failure).

Also holds a tiny reference tokeniser/renderer used to cross-check `typing` golden cases
(options gh/q/c/s_order/z_order + raw map overrides). It is NOT the product engine.
"""
import json, sys, tomllib, unicodedata
from pathlib import Path

D = Path(__file__).parent
errors = []
def err(msg): errors.append(msg)

ARABIC_ONLY = {"\u064a", "\u0643", "\u0629"}
ZWNJ = "\u200c"
def ok_cp(c):
    cp = ord(c)
    return c == ZWNJ or 0x0600 <= cp <= 0x06FF or 0x0750 <= cp <= 0x077F or 0xFB50 <= cp <= 0xFDFF or 0xFE70 <= cp <= 0xFEFF

def check_map(where, m, allow_arabic=False):
    for sec in ("singles", "digraphs", "trigraphs"):
        for k, cands in m.get(sec, {}).items():
            n = {"singles": 1, "digraphs": 2, "trigraphs": 3}[sec]
            if len(k) != n or not all(32 < ord(ch) < 127 for ch in k): err(f"{where}: bad key {sec}.{k!r}")
            if not isinstance(cands, list) or not cands: err(f"{where}: {k}: empty candidates"); continue
            if len(set(cands)) != len(cands): err(f"{where}: {k}: duplicate candidates")
            for c in cands:
                if len(c) != 1 or unicodedata.normalize("NFC", c) != c: err(f"{where}: {k}: {c!r} not one NFC code point")
                elif not ok_cp(c): err(f"{where}: {k}: {c!r} (U+{ord(c):04X}) outside Persian/Arabic blocks")
                elif c in ARABIC_ONLY and not allow_arabic: err(f"{where}: {k}: Arabic-only form U+{ord(c):04X}")

def load_preset(name):
    return tomllib.loads((D / "presets" / f"{name}.toml").read_text(encoding="utf-8"))

# ---- reference renderer ----
def resolve(cfg):
    """cfg: preset name or {"preset":..,"toml":..}; returns (options, map dict key->list)."""
    if isinstance(cfg, str): cfg = {"preset": cfg, "toml": ""}
    p = load_preset(cfg["preset"]); u = tomllib.loads(cfg["toml"])
    opts = {**p["options"], **u.get("options", {})}
    m = dict(p["map"]["singles"]); m.update(p["map"]["digraphs"]); m.update(p["map"].get("trigraphs", {}))
    if "s_order" in opts: m["s"] = list(opts["s_order"])
    if "z_order" in opts: m["z"] = list(opts["z_order"])
    def promote(key, ch):
        m[key] = [ch] + [x for x in m[key] if x != ch]
    promote("gh", "ق" if opts["gh"] == "qaf" else "غ")
    promote("q", "ق" if opts["q"] == "qaf" else "غ")
    if opts["c"] == "k": m["c"] = ["ک"]
    elif opts["c"] == "s": m["c"] = list(m["s"])
    else: m.pop("c", None)
    for sec in ("singles", "digraphs", "trigraphs"):
        m.update(u.get("map", {}).get(sec, {}))
    return opts, m

def tokens(latin, m):
    out, i = [], 0
    while i < len(latin):
        for n in (3, 2, 1):
            k = latin[i:i+n]
            if len(k) == n and k in m: out.append((k, m[k])); i += n; break
        else:
            out.append((latin[i], None)); i += 1   # unmapped letter (lone c): visible literal
    return out

def render(latin, m):
    return "".join(c[0] if c else k for k, c in tokens(latin, m))

def last_alts(latin, m):
    t = tokens(latin, m)
    return list(t[-1][1]) if t and t[-1][1] and len(t[-1][1]) > 1 else None

# ---- checks ----
def main():
    names = {}
    for f in sorted((D / "presets").glob("*.toml")):
        try: p = tomllib.loads(f.read_text(encoding="utf-8"))
        except Exception as e: err(f"{f.name}: {e}"); continue
        check_map(f.name, p.get("map", {}))
        s = p["map"]["singles"]
        for ch in "abcdefghijklmnopqrstuvwxyz'":
            if ch != "c" and ch not in s: err(f"{f.name}: missing single {ch!r}")
        if s["a"] != s["e"]: err(f"{f.name}: e must equal a")
        if f.stem == "minimal":
            for sec in ("singles", "digraphs"):
                for k, v in p["map"][sec].items():
                    if len(v) != 1: err(f"minimal: {k} has {len(v)} candidates")
    q = load_preset("qamooscheh")["map"]
    exp_d = {"kh": ["خ"], "gh": ["غ", "ق"], "ch": ["چ"], "sh": ["ش"], "zh": ["ژ"]}
    if q["digraphs"] != exp_d: err("qamooscheh digraphs differ from web map")
    try:
        pu = tomllib.loads((D / "punctuation.toml").read_text(encoding="utf-8"))
        for i in range(10):
            if pu["digits"]["persian"][str(i)] != chr(0x06F0 + i): err(f"digit {i}")
        for k, cp in {",": 0x060C, ";": 0x061B, "?": 0x061F, "%": 0x066A}.items():
            if pu["punctuation"]["persian"][k] != chr(cp): err(f"punct {k}")
    except Exception as e: err(f"punctuation.toml: {e}")
    cfgs = {"qamooscheh", "minimal"}
    seen = set()
    for f in sorted((D / "golden").glob("*.json")):
        try: g = json.loads(f.read_text(encoding="utf-8"))
        except Exception as e: err(f"{f.name}: {e}"); continue
        if g.get("version") != 1 or not isinstance(g.get("cases"), list): err(f"{f.name}: bad envelope"); continue
        for c in g["cases"]:
            n = c.get("name"); w = f"{f.name}:{n}"
            if not n or n in seen: err(f"{w}: missing/duplicate name")
            seen.add(n)
            t = c.get("type")
            cfg = c.get("config")
            if t in ("typing", "session"):
                if isinstance(cfg, str):
                    if cfg not in cfgs: err(f"{w}: unknown preset")
                elif not (isinstance(cfg, dict) and set(cfg) == {"preset", "toml"}): err(f"{w}: bad config")
                else:
                    try: tomllib.loads(cfg["toml"])
                    except Exception as e: err(f"{w}: config toml: {e}")
            if t == "typing":
                if not (isinstance(c.get("input"), str) and isinstance(c.get("preedit_after_each"), list)
                        and len(c["preedit_after_each"]) == len(c["input"]) and c["final_preedit"] == c["preedit_after_each"][-1]):
                    err(f"{w}: typing shape"); continue
                _, m = resolve(cfg)
                for i in range(len(c["input"])):
                    if render(c["input"][:i+1], m) != c["preedit_after_each"][i]: err(f"{w}: step {i} reference mismatch")
                if "candidates_after_last" in c and c["candidates_after_last"] != last_alts(c["input"], m):
                    err(f"{w}: candidates_after_last mismatch")
            elif t == "session":
                if not (isinstance(c.get("keys"), list) and isinstance(c.get("committed"), str) and isinstance(c.get("preedit"), str)): err(f"{w}: session shape")
                known = {"Space","Enter","Escape","Backspace","Tab","Up","Down","Left","Right","ShiftSpace"}
                for k in c.get("keys", []):
                    if k not in known and len(k) != 1: err(f"{w}: bad key {k!r}")
            elif t == "config_error":
                e = c.get("expect", {})
                if e.get("fallback") not in ("entry", "whole_file", "none") or not isinstance(e.get("messages_contain"), list): err(f"{w}: expect shape")
                try: tomllib.loads(c["user_config_toml"]); parsed = True
                except Exception: parsed = False
                if (e.get("fallback") == "whole_file") == parsed: err(f"{w}: fallback whole_file iff TOML fails to parse")
            else: err(f"{w}: unknown type {t!r}")
    if errors:
        print("\n".join(errors)); print(f"FAILED: {len(errors)} problem(s)"); sys.exit(1)
    print(f"OK: {len(seen)} golden cases, presets and punctuation valid")

if __name__ == "__main__": main()
