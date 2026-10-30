#!/bin/bash
# Documentation checks: relative markdown links resolve, removed documents are
# not referenced, and the docs index covers every tracked markdown file.
#
# Run from the repository root: ./scripts/check-docs.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

python3 - "$ROOT" <<'PY'
import os
import re
import subprocess
import sys
from pathlib import Path

root = Path(sys.argv[1])
SKIP_DIRS = {
    ".git", ".cargo-home", "target", "dist", "local", ".e2e", ".uicheck",
    ".shots", ".first-run-check", ".tmp-cache", "node_modules", "__pycache__",
}
LEGACY = {
    "ux.md": "replaced by docs/design.md",
    "ux.ru.md": "replaced by docs/design.ru.md",
    "PLAN.md": "removed; direction lives in docs/roadmap.md",
    "extension.md": "removed; the GNOME Shell applet is gone (ADR 0004)",
}

try:
    listing = subprocess.run(
        ["git", "ls-files", "*.md", "*.MD"],
        cwd=root, capture_output=True, text=True, check=True,
    ).stdout.splitlines()
except Exception:
    listing = []
files = [root / p for p in listing if (root / p).is_file()]
if not files:
    files = [
        path
        for path in root.rglob("*.md")
        if not any(part in SKIP_DIRS for part in path.relative_to(root).parts)
    ]

link_re = re.compile(r"\[[^\]]*\]\(\s*(<[^>]+>|[^)\s]+)(?:\s+\"[^\"]*\")?\s*\)")
html_re = re.compile(r'(?:\bsrc|\bsrcset)="([^"]+)"')
code_fence_re = re.compile(r"```.*?```", re.S)
inline_code_re = re.compile(r"`[^`]*`")


def check_target(rel, base, target):
    """Return an error string for a local target that does not resolve."""
    if not target or target.startswith(("http://", "https://", "mailto:", "data:", "#")):
        return None
    target = target.split("#", 1)[0].split("?", 1)[0]
    if not target:
        return None
    target = target.replace("%20", " ")
    resolved = (base / target).resolve()
    try:
        resolved.relative_to(root)
    except ValueError:
        return f"{rel}: link escapes the repository: {target}"
    if not resolved.exists():
        return f"{rel}: missing link target: {target}"
    if target.rsplit("/", 1)[-1] in LEGACY:
        return f"{rel}: legacy document: {target} ({LEGACY[target.rsplit('/', 1)[-1]]})"
    return None

errors = []
for path in sorted(files):
    rel = path.relative_to(root).as_posix()
    text = path.read_text(encoding="utf-8", errors="replace")
    text = code_fence_re.sub("", text)
    text = inline_code_re.sub("", text)
    for match in link_re.finditer(text):
        target = match.group(1).strip()
        if target.startswith("<") and target.endswith(">"):
            target = target[1:-1]
        error = check_target(rel, path.parent, target)
        if error:
            errors.append(error)
    for match in html_re.finditer(text):
        for candidate in match.group(1).split(","):
            target = candidate.strip().split(" ")[0]
            error = check_target(rel, path.parent, target)
            if error:
                errors.append(error)

# Every tracked markdown file must be reachable from the docs index or the
# README, so a new document cannot be orphaned by accident.
index_text = (root / "docs/README.md").read_text(encoding="utf-8") if (root / "docs/README.md").exists() else ""
readme_text = (root / "README.md").read_text(encoding="utf-8") if (root / "README.md").exists() else ""
reachable = set()
for text, base in ((index_text, root / "docs"), (readme_text, root)):
    for match in link_re.finditer(text):
        target = match.group(1).strip("<>").split("#", 1)[0].split("?", 1)[0]
        if not target or target.startswith(("http://", "https://", "mailto:")):
            continue
        resolved = (base / target).resolve()
        reachable.add(resolved)
        if resolved.is_dir():
            reachable.update(p.resolve() for p in resolved.rglob("*.md"))
for path in files:
    if path.name in {"README.md", "CONTRIBUTING.md", "CONTRIBUTING.ru.md",
                     "SECURITY.md", "THIRD_PARTY.md", "CHANGELOG.md", "AGENTS.md"}:
        continue
    if path.parent == root:
        continue
    if path.resolve() not in reachable:
        errors.append(f"{path.relative_to(root).as_posix()}: not linked from docs/README.md")

if errors:
    print("documentation check failed:", file=sys.stderr)
    for error in errors:
        print(f"  - {error}", file=sys.stderr)
    sys.exit(1)
print(f"documentation check passed ({len(files)} markdown files)")
PY
