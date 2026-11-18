"""Remove the imports rustc reports as unused that `cargo fix` leaves alone (trait imports such as `slint::Model`
after a module split). Reads `cargo check --message-format short` output on stdin.

    cargo check -p lag-app --message-format short 2>&1 | python -I tools/drop_unused_imports.py
"""
import re
import sys

edits = {}
pending = None  # names of the last "unused import" warning in the long format, waiting for its `-->` line
for line in sys.stdin:
    line = line.strip()
    m = re.match(r"^(.+?\.rs):(\d+):(\d+): warning: unused imports?: (.+)$", line)  # --message-format short
    if m:
        edits.setdefault(m.group(1), []).append((int(m.group(2)), re.findall(r"`([^`]+)`", m.group(4))))
        continue
    m = re.match(r"^warning: unused imports?: (.+)$", line)
    if m:
        pending = re.findall(r"`([^`]+)`", m.group(1))
        continue
    m = re.match(r"^--> (.+?\.rs):(\d+):(\d+)$", line)
    if m and pending:
        path = m.group(1)
        key = (int(m.group(2)), pending)
        if key not in edits.setdefault(path, []):
            edits[path].append(key)
        pending = None

for path, items in edits.items():
    lines = open(path, encoding="utf-8").read().split("\n")
    for ln, names in sorted(items, reverse=True):
        i = ln - 1
        # the whole `use ...;` statement starting at or above line i
        start = i
        while not lines[start].lstrip().startswith("use "):
            start -= 1
        end = start
        while not lines[end].rstrip().endswith(";"):
            end += 1
        stmt = "\n".join(lines[start:end + 1])
        for name in names:
            short = name.split("::")[-1]
            if stmt.strip() in (f"use {name};", f"use {short};") or re.fullmatch(rf"\s*use [\w:]*{re.escape(short)};\s*", stmt):
                stmt = ""
                break
            stmt = re.sub(rf"(?<![\w:]){re.escape(short)}\s*,\s*", "", stmt, count=1) if re.search(
                rf"(?<![\w:]){re.escape(short)}\s*,", stmt) else re.sub(rf",\s*{re.escape(short)}(?![\w:])", "", stmt, count=1)
        stmt = re.sub(r"\{\s*([\w:]+)\s*\}", r"\1", stmt)  # use a::{B}; -> use a::B;
        lines[start:end + 1] = [stmt] if stmt else []
    open(path, "w", encoding="utf-8").write("\n".join(lines))
    print("fixed", path, len(items))
