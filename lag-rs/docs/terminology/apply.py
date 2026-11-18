"""Apply the approved terminology to crates/lag-core/langs/*.json. Idempotent: run it again any time.

Sources (later wins):
  1. proposals.json       en_new / uk_new for every key (owner approved all proposals on 2026-11-16)
  2. decisions.json       optional export of the review page's db `edits/<key>` (status/en/uk)
  3. translations.json    optional {lang: {key: text}} for the other 15 languages

Only the values of keys listed in those sources change. Every other key (e.g. keys other packages added),
the key order and the file format (2-space indent, raw UTF-8, trailing newline as found) are preserved.
Usage: python -I apply.py
"""
import json
import pathlib

HERE = pathlib.Path(__file__).resolve().parent
LANGS = HERE.parent.parent / "crates" / "lag-core" / "langs"


def load(name, default):
    p = HERE / name
    return json.loads(p.read_text(encoding="utf-8")) if p.exists() else default


proposals = load("proposals.json", [])
decisions = load("decisions.json", {})
translations = load("translations.json", {})

approved = {"en": {}, "uk": {}}
for p in proposals:
    approved["en"][p["key"]] = p["en_new"]
    approved["uk"][p["key"]] = p["uk_new"]
for key, d in decisions.items():
    if d.get("status") == "kept":
        src = next((p for p in proposals if p["key"] == key), None)
        if src and src.get("en_old"):
            approved["en"][key], approved["uk"][key] = src["en_old"], src["uk_old"]
    elif d.get("status") in ("accepted", "edited"):
        approved["en"][key], approved["uk"][key] = d["en"], d["uk"]
for lang, values in translations.items():
    approved.setdefault(lang, {}).update(values)

for path in sorted(LANGS.glob("*.json")):
    lang = path.stem
    changes = approved.get(lang)
    if not changes:
        continue
    raw = path.read_text(encoding="utf-8")
    data = json.loads(raw)
    changed = 0
    for key, text in changes.items():
        if data.get(key) != text:
            data[key] = text  # existing keys keep their position; new keys append
            changed += 1
    out = json.dumps(data, ensure_ascii=False, indent=2)
    if raw.endswith("\n"):
        out += "\n"
    if out != raw:
        path.write_text(out, encoding="utf-8", newline="\n")
    print(f"{lang}: {changed} value(s) updated, {len(data)} keys")
