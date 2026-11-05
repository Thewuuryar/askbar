#!/bin/sh
# Checks the markdown files:
#   1. every relative link points at a file that exists, and a #anchor at a heading in that file;
#   2. every ADR in docs/adr/ has a row in docs/adr/README.md, and the row's Status matches the ADR's "Status:" line.
# Covers every tracked *.md file (all *.md files outside node_modules and target when git is not available).
# Usage: scripts/check-docs.sh
cd "$(dirname "$0")/.." || exit 2
python3 - <<'PY'
import os, re, subprocess, sys

try:
    files = subprocess.run(["git", "ls-files", "*.md"], capture_output=True, text=True, check=True).stdout.split()
except (OSError, subprocess.CalledProcessError):
    files = []
    for root, dirs, names in os.walk("."):
        dirs[:] = [d for d in dirs if d not in ("node_modules", "target", ".git")]
        files += [os.path.normpath(os.path.join(root, n)) for n in names if n.endswith(".md")]
files = [f for f in files if os.path.exists(f)]

def strip_code(text):
    text = re.sub(r"```.*?```", "", text, flags=re.S)  # code samples may show example links
    return re.sub(r"`[^`\n]*`", "", text)              # so may inline code

def slug(heading):
    heading = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", heading)  # a link in a heading keeps its text
    heading = re.sub(r"[`*_]", "", heading).strip().lower()
    return re.sub(r"[^\w\- ]", "", heading).replace(" ", "-")

texts = {f: strip_code(open(f, encoding="utf-8").read()) for f in files}
anchors = {}
for f, t in texts.items():
    seen, found = {}, set()
    for m in re.finditer(r"^#{1,6}\s+(.*?)\s*#*\s*$", t, re.M):
        s = slug(m.group(1))
        n = seen.get(s, 0)
        seen[s] = n + 1
        found.add(s if n == 0 else f"{s}-{n}")
    anchors[f] = found

bad = 0
for f, t in texts.items():
    for m in re.finditer(r"\[[^\]]*\]\(<?([^)\s>]+)>?\)", t):
        target = m.group(1)
        if re.match(r"^[a-z]+:", target):
            continue
        path, _, anchor = target.partition("#")
        full = os.path.normpath(os.path.join(os.path.dirname(f), path)) if path else f
        if not os.path.exists(full):
            print(f"{f}: broken link {target}")
            bad += 1
        elif anchor and full.endswith(".md") and full in anchors and anchor not in anchors[full]:
            print(f"{f}: no heading for anchor {target}")
            bad += 1

# ADR index (ADR 0027)
rows = {}
if os.path.exists("docs/adr/README.md"):
    for m in re.finditer(r"^\|\s*\[(\d{4})\]\([^)]*\)\s*\|[^|]*\|\s*([^|]*?)\s*\|", open("docs/adr/README.md", encoding="utf-8").read(), re.M):
        rows[m.group(1)] = m.group(2)
    nums = []
    for name in sorted(os.listdir("docs/adr")):
        m = re.match(r"^(\d{4})-.*\.md$", name)
        if not m or m.group(1) == "0000":
            continue
        n = m.group(1)
        nums.append(int(n))
        sm = re.search(r"^Status:\s*(\w+)", open(os.path.join("docs/adr", name), encoding="utf-8").read(), re.M)
        if n not in rows:
            print(f"docs/adr/README.md: no row for ADR {n}"); bad += 1
        elif not sm:
            print(f"docs/adr/{name}: no Status line"); bad += 1
        elif sm.group(1) != rows[n].split()[0]:
            print(f"docs/adr/{name}: Status {sm.group(1)} but README says {rows[n]}"); bad += 1
    if nums and nums != list(range(1, len(nums) + 1)):
        print("docs/adr: numbers are not contiguous from 0001"); bad += 1
    for n in rows:
        if int(n) not in nums:
            print(f"docs/adr/README.md: row for ADR {n} has no file"); bad += 1

print("docs ok" if not bad else f"{bad} problem(s)")
sys.exit(1 if bad else 0)
PY
