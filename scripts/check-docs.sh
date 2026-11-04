#!/bin/sh
# Fails if a relative markdown link in README.md, CONTRIBUTING.md, SECURITY.md, AGENTS.md or docs/ points at a file that does not exist.
# Usage: scripts/check-docs.sh
cd "$(dirname "$0")/.." || exit 2
python3 - <<'PY'
import os, re, sys
files = [f for f in ("README.md", "CONTRIBUTING.md", "SECURITY.md", "AGENTS.md") if os.path.exists(f)]
for root, _, names in os.walk("docs"):
    files += [os.path.join(root, n) for n in names if n.endswith(".md")]
bad = 0
for f in files:
    text = open(f, encoding="utf-8").read()
    text = re.sub(r"```.*?```", "", text, flags=re.S)  # code samples may show example links
    for m in re.finditer(r"\[[^\]]*\]\(([^)\s]+)\)", text):
        target = m.group(1)
        if re.match(r"^[a-z]+:|^#|^mailto:", target):
            continue
        path = target.split("#")[0]
        if not path:
            continue
        full = os.path.normpath(os.path.join(os.path.dirname(f), path))
        if not os.path.exists(full):
            print(f"{f}: broken link {target}")
            bad += 1
print("docs links ok" if not bad else f"{bad} broken link(s)")
sys.exit(1 if bad else 0)
PY
