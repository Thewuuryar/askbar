#!/bin/sh
# Checks the reference pages under docs/reference (plan step 7.1a):
#   1. every page has front matter with the five keys id, title, summary, keywords and settings (id, title and summary
#      not empty, keywords not empty; settings may be []);
#   2. every id is unique, and is the page's path under docs/reference without .md (index.md is `index`);
#   3. every relative link resolves to an existing file, and a #anchor to a heading in that file;
#   4. every help topic in HELP_TOPICS (services/admin/src/constants.rs) is `<page id>#<heading>`, and that page has a
#      heading with that anchor (the anchor is the heading's text in the same form as check-docs.sh makes it).
# Usage: scripts/check-reference.sh   (also run by scripts/check-docs.sh)
cd "$(dirname "$0")/.." || exit 2
python3 - <<'PY'
import os, re, sys

ROOT = "docs/reference"
CONSTANTS = "services/admin/src/constants.rs"
KEYS = ("id", "title", "summary", "keywords", "settings")

def strip_fences(text):
    return re.sub(r"```.*?```", "", text, flags=re.S)

def strip_code(text):
    text = strip_fences(text)
    return re.sub(r"`[^`\n]*`", "", text)

def slug(heading):
    heading = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", heading)  # a link in a heading keeps its text
    heading = re.sub(r"[`*_]", "", heading).strip().lower()
    return re.sub(r"[^\w\- ]", "", heading).replace(" ", "-")

def anchors_of(text):
    seen, found = {}, set()
    for m in re.finditer(r"^#{1,6}\s+(.*?)\s*#*\s*$", strip_fences(text), re.M):
        s = slug(m.group(1))
        n = seen.get(s, 0)
        seen[s] = n + 1
        found.add(s if n == 0 else f"{s}-{n}")
    return found

def front_matter(text):
    m = re.match(r"---\n(.*?)\n---\n", text, re.S)
    if not m:
        return None
    fields = {}
    for line in m.group(1).splitlines():
        k = re.match(r"^([a-z_]+):\s*(.*)$", line)
        if k:
            fields[k.group(1)] = k.group(2).strip()
    return fields

bad = 0
def problem(msg):
    global bad
    print(msg)
    bad += 1

pages = {}  # path -> text
for dirpath, dirs, names in os.walk(ROOT):
    for n in sorted(names):
        if n.endswith(".md"):
            path = os.path.join(dirpath, n)
            pages[path] = open(path, encoding="utf-8").read()

if not pages:
    problem(f"{ROOT}: no pages")

ids = {}  # id -> path
anchors = {}
for path, text in sorted(pages.items()):
    anchors[path] = anchors_of(text)
    fm = front_matter(text)
    if fm is None:
        problem(f"{path}: no front matter (--- lines at the top)")
        continue
    for key in KEYS:
        if key not in fm:
            problem(f"{path}: front matter has no '{key}'")
    for key in ("id", "title", "summary", "keywords"):
        if key in fm and not fm[key]:
            problem(f"{path}: front matter '{key}' is empty")
    pid = fm.get("id", "")
    want = os.path.relpath(path, ROOT)[: -len(".md")]
    if pid and pid != want:
        problem(f"{path}: id '{pid}' is not the page's path ('{want}')")
    if pid:
        if pid in ids:
            problem(f"{path}: id '{pid}' is also used by {ids[pid]}")
        else:
            ids[pid] = path

for path, text in sorted(pages.items()):
    for m in re.finditer(r"\[[^\]]*\]\(<?([^)\s>]+)>?\)", strip_code(text)):
        target = m.group(1)
        if re.match(r"^[a-z]+:", target):
            continue
        rel, _, anchor = target.partition("#")
        full = os.path.normpath(os.path.join(os.path.dirname(path), rel)) if rel else path
        if not os.path.isfile(full):
            problem(f"{path}: link {target} does not resolve to a file")
        elif anchor and full.endswith(".md"):
            if full in anchors:
                found = anchors[full]
            else:
                with open(full, encoding="utf-8") as f:
                    found = anchors_of(f.read())
            if anchor not in found:
                problem(f"{path}: link {target} has no heading for '#{anchor}'")

try:
    code = open(CONSTANTS, encoding="utf-8").read()
except OSError:
    code = ""
m = re.search(r"pub const HELP_TOPICS\b[^=]*=\s*\[(.*?)\];", code, re.S)
if not m:
    problem(f"{CONSTANTS}: no HELP_TOPICS array")
else:
    topics = re.findall(r'"([^"]*)"', m.group(1))
    if not topics:
        problem(f"{CONSTANTS}: HELP_TOPICS is empty")
    for topic in topics:
        page, hash_, anchor = topic.partition("#")
        if not hash_ or not anchor:
            problem(f"{CONSTANTS}: help topic '{topic}' is not 'page#heading'")
            continue
        path = ids.get(page)
        if path is None:
            problem(f"{CONSTANTS}: help topic '{topic}' names no reference page with id '{page}'")
        elif anchor not in anchors[path]:
            problem(f"{CONSTANTS}: help topic '{topic}' has no heading '#{anchor}' in {path}")

print("reference ok" if not bad else f"{bad} problem(s)")
sys.exit(1 if bad else 0)
PY
