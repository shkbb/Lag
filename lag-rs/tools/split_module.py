"""Split one big Rust source file into a module directory, moving code VERBATIM.

Used for the 2026-11-18 code-quality pass (views/editor.rs, library.rs, settings.rs, player.rs). Each part is a
list of line ranges; every range start is checked against an expected line prefix, so a file that changed since
the spec was written aborts instead of being cut mid-function. Private top-level items become `pub(super)` so
the sibling modules can use them; every module gets the original file's imports plus glob imports of its
siblings, and `cargo fix` removes what is unused afterwards. Logic is never rewritten here.

    python -I tools/split_module.py <spec.py>

The spec file defines SRC (the .rs file), DOC (root doc lines), MODULES = [(name, doc, [(a, b, prefix), ...])],
ROOT = [(a, b, prefix), ...] (kept in the root file after the module list), TESTS = (a, b) of a `mod tests {`
block or None, ROOT_EXPORTS = ["pub(crate) use x::y;", ...].
"""
import os
import re
import sys

spec = {}
exec(open(sys.argv[1], encoding="utf-8").read(), spec)
SRC = spec["SRC"]
L = open(SRC, encoding="utf-8").read().split("\n")


def text(a, b, prefix=None):
    if prefix is not None:
        assert L[a - 1].startswith(prefix), (SRC, a, prefix, L[a - 1])
    return "\n".join(L[a - 1:b])


# the original imports: every `use` statement before the first item, including multi-line ones
header, i = [], 0
while i < len(L) and not re.match(r"^(pub |const |static |fn |struct |enum |impl|#\[|thread_local|type |trait |mod )", L[i]):
    if L[i].startswith("use "):
        stmt = [L[i]]
        while not stmt[-1].rstrip().endswith(";"):
            i += 1
            stmt.append(L[i])
        header.append("\n".join(stmt))
    i += 1
IMPORTS = "\n".join(header) + "\n"

names = [m[0] for m in spec["MODULES"]]
used = set()


def is_lead(n):
    """Doc comment / attribute line: belongs to the item BELOW it."""
    s = L[n - 1]
    return s.startswith("///") or s.startswith("#[")


def take(ranges):
    parts = []
    for a, b, prefix in ranges:
        if prefix is not None:
            text(a, b, prefix)  # check the spec's line first
        while a > 1 and is_lead(a - 1):  # a range owns the docs/attributes above its first item
            a -= 1
        while b > a and (is_lead(b) or not L[b - 1].strip()):  # ...and not those of the next item
            b -= 1
        prefix = None
        for n in range(a, b + 1):
            assert n not in used, (SRC, "line used twice", n)
            used.add(n)
        parts.append(text(a, b, prefix))
    return "\n\n".join(p.strip("\n") for p in parts)


def widen(body):
    """Private top-level items -> pub(super); private fields of top-level structs -> pub(super)."""
    out = []
    in_struct = False
    for ln in body.split("\n"):
        if re.match(r"^(fn|struct|enum|const|static|type|trait) ", ln):
            ln = "pub(super) " + ln
        if re.match(r"^(pub(\([a-z]+\))? )?struct \w+.*\{\s*$", ln):
            in_struct = True
        elif in_struct and ln.startswith("}"):
            in_struct = False
        elif in_struct and re.match(r"^    \w+: ", ln):
            ln = "    pub(super) " + ln[4:]
        out.append(ln)
    return "\n".join(out)


base = os.path.splitext(SRC)[0]
os.makedirs(base, exist_ok=True)
for name, doc, ranges in spec["MODULES"]:
    body = widen(take(ranges))
    body = re.sub(r"\n    fn ", "\n    pub(super) fn ", body) if name in spec.get("PUB_METHODS", []) else body
    sib = ", ".join(f"{n}::*" for n in names if n != name)
    out = f"{doc.rstrip()}\n\n{IMPORTS}#[allow(unused_imports)]\nuse super::{{{sib}}};\nuse super::*;\n\n{body}\n"
    open(f"{base}/{name}.rs", "w", encoding="utf-8", newline="\n").write(out)

tests = ""
if spec.get("TESTS"):
    a, b = spec["TESTS"]
    assert L[a - 1].startswith("mod tests {") and L[b - 1] == "}", (L[a - 1], L[b - 1])
    for n in range(a - 1, b + 1):
        used.add(n)
    body = "\n".join(l[4:] if l.startswith("    ") else l for l in L[a:b - 1])
    body = body.replace("use super::*;\n", "", 1)
    sib = ", ".join(f"{n}::*" for n in names)
    open(f"{base}/tests.rs", "w", encoding="utf-8", newline="\n").write(
        f"use super::{{{sib}}};\nuse super::*;\n\n{body.strip(chr(10))}\n")

root_body = take(spec["ROOT"])
mods = "\n".join(f"mod {n};" for n in sorted(names + (["tests"] if tests or spec.get("TESTS") else [])))
mods = mods.replace("mod tests;", "#[cfg(test)]\nmod tests;")
globs = "\n".join(f"use {n}::*;" for n in names)
root = ("\n".join(spec["DOC"]) + "\n\n" + IMPORTS + "\n" + mods + "\n\n" + "\n".join(spec.get("ROOT_EXPORTS", [])) +
        "\n#[allow(unused_imports)]\n" + globs.replace("\n", "\n#[allow(unused_imports)]\n") + "\n\n" + root_body + "\n")
open(SRC, "w", encoding="utf-8", newline="\n").write(root)

# every non-blank line of the source must have gone somewhere
lost = [n for n in range(1, len(L) + 1) if n not in used and L[n - 1].strip() and not L[n - 1].startswith("use ")
        and not L[n - 1].startswith("//!")]
cont = [n for n in lost if not re.match(r"^\s", L[n - 1]) or True]
print("modules:", names, "| unplaced non-import lines:", lost[:40])
