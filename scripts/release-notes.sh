#!/usr/bin/env bash
# Prints a release's notes for the GitHub release page. The notes file (docs/releases/<tag>.md) uses relative links, which
# scripts/check-docs.sh checks; a release page has no repository path to resolve them against, so each one is rewritten to an
# absolute link at the tag: https://github.com/<repo>/blob/<tag>/<path> (tree/ for a folder). Run by .github/workflows/images.yml.
#
# Usage: scripts/release-notes.sh <tag> [--paths | --out <file>]
#   <tag>    the release tag, for example v1.0.0-rc.1; the notes are docs/releases/<tag>.md
#   --out    write the notes to <file> (for a test; the notes go to stdout by default)
#   --paths  print the repository path each link points to, one per line, instead of the notes
# It fails if the notes file is missing, if a link points to a path that is not in the repository, or if a link is only a
# `#anchor` (it would not work on the release page).
#   ASKBAR_RELEASE_REPO  owner/name (default Thewuuryar/askbar)
set -euo pipefail
cd "$(dirname "$0")/.."
TAG=${1:?usage: scripts/release-notes.sh <tag> [--paths | --out <file>]}
MODE=${2:-notes}
OUTFILE=${3:-}
NOTES="docs/releases/$TAG.md"
[ -f "$NOTES" ] || { echo "release-notes.sh: $NOTES does not exist" >&2; exit 1; }
python3 - "$NOTES" "$TAG" "$MODE" "${ASKBAR_RELEASE_REPO:-Thewuuryar/askbar}" "$OUTFILE" <<'PY'
import os, posixpath, re, sys
notes, tag, mode, repo, outfile = sys.argv[1:6]
text = open(notes, encoding="utf-8").read()
base = posixpath.dirname(notes)
bad, paths = [], []
def fix(m):
    label, target = m.group(1), m.group(2)
    if re.match(r"^[A-Za-z][A-Za-z0-9+.-]*:", target):
        return m.group(0)
    if target.startswith("#"):
        bad.append(f"{target}: an anchor alone does not work on the release page")
        return m.group(0)
    path, _, frag = target.partition("#")
    repo_path = posixpath.normpath(posixpath.join(base, path))
    if repo_path.startswith("..") or not os.path.exists(repo_path):
        bad.append(f"{target}: {repo_path} is not in the repository")
        return m.group(0)
    paths.append(repo_path)
    kind = "tree" if os.path.isdir(repo_path) else "blob"
    url = f"https://github.com/{repo}/{kind}/{tag}/{repo_path.rstrip('/')}" + (f"#{frag}" if frag else "")
    return f"[{label}]({url})"
# Links outside fenced code blocks only.
out, fenced = [], False
for line in text.splitlines(keepends=True):
    if line.lstrip().startswith("```"):
        fenced = not fenced
    out.append(line if fenced else re.sub(r"\[([^\]]*)\]\(([^)\s]+)\)", fix, line))
if bad:
    sys.exit("release-notes.sh: " + "; ".join(bad))
body = "\n".join(sorted(set(paths))) + "\n" if mode == "--paths" else "".join(out)
if mode == "--out":
    open(outfile, "w", encoding="utf-8").write(body)
else:
    sys.stdout.write(body)
PY
