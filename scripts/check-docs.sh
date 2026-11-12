#!/bin/sh
# Checks the markdown files:
#   1. every relative link points at a file that exists, and a #anchor at a heading in that file;
#   2. every ADR in docs/adr/ has a row in docs/adr/README.md, and the row's Status matches the ADR's "Status:" line;
#   3. ADR 0001: every ADR but a stub has an Amendments section; a Kind is component or register; a Retired or
#      Deferred ADR says when and why; a "Merged into NNNN" stub has a Merged: line and names an ADR that exists,
#      is not a stub and lists it in its Amendments; no ADR has an "Amends:" line or an "amended by" status.
#   4. ADR 0016: every Defend row has a row in the test table, and every test named there exists (`pending` is allowed until 1.0).
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

# ADR index (ADR 0001)
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
        elif sm.group(1) != rows[n].split()[0].rstrip(";,"):
            print(f"docs/adr/{name}: Status {sm.group(1)} but README says {rows[n]}"); bad += 1
    # ADR 0001: kinds, amendments, stubs, retired and deferred ADRs.
    adrs = {}
    for name in sorted(os.listdir("docs/adr")):
        m = re.match(r"^(\d{4})-.*\.md$", name)
        if m and m.group(1) != "0000":
            adrs[m.group(1)] = (name, open(os.path.join("docs/adr", name), encoding="utf-8").read())
    for n, (name, text) in adrs.items():
        where = f"docs/adr/{name}"
        kind = re.search(r"^Kind:\s*(\S+)", text, re.M)
        if kind and kind.group(1) not in ("component", "register"):
            print(f"{where}: Kind {kind.group(1)} is not component or register"); bad += 1
        status = (re.search(r"^Status:\s*(.*)$", text, re.M) or [None, ""])[1]
        merged = re.match(r"Merged into (\d{4})\s*$", status)
        if status.startswith("Merged") and not merged:
            print(f"{where}: Status {status!r} should be 'Merged into NNNN'"); bad += 1
        if merged:
            if not re.search(r"^Merged:\s*\S", text, re.M):
                print(f"{where}: a merged ADR needs a 'Merged:' line with the date and where its content went"); bad += 1
            into = adrs.get(merged.group(1))
            if not into:
                print(f"{where}: merged into {merged.group(1)}, which does not exist"); bad += 1
            elif re.search(r"^Status:\s*Merged", into[1], re.M):
                print(f"{where}: merged into {merged.group(1)}, which is itself a stub"); bad += 1
            else:
                amends = into[1].split("## Amendments", 1)[1] if "## Amendments" in into[1] else ""
                if not re.search(rf"\b{n}\b", amends):
                    print(f"{where}: merged into {into[0]}, whose Amendments do not mention {n}"); bad += 1
        elif not re.search(r"^## Amendments\s*$", text, re.M):
            print(f"{where}: needs an '## Amendments' section (empty until the first amendment)"); bad += 1
        for word in ("Retired", "Deferred"):
            if status.startswith(word) and not re.search(rf"^{word}:\s*\S", text, re.M):
                print(f"{where}: Status {word} needs a '{word}:' line with the date and the reason"); bad += 1
        # Partial amendments are gone (0001 rule 13): a change is made in the ADR itself.
        if re.search(r"^Amends:", text, re.M) or "amended by" in status:
            print(f"{where}: has an 'Amends:' line or an 'amended by' status; amend the ADR in place instead (ADR 0001)"); bad += 1
    if nums and nums != list(range(1, len(nums) + 1)):
        print("docs/adr: numbers are not contiguous from 0001"); bad += 1
    for n in rows:
        if int(n) not in nums:
            print(f"docs/adr/README.md: row for ADR {n} has no file"); bad += 1

# Defend table (ADR 0016, D36): every Defend row has a row in "Where each Defend row is tested", and every test
# named there (in backticks) exists in a tracked file that is not markdown. "pending, PR <letter>" is allowed until 1.0.
adr16 = "docs/adr/0016-risk-register.md"
if os.path.exists(adr16):
    t16 = open(adr16, encoding="utf-8").read()
    def first_cells(block):
        return [m.group(1).strip() for m in re.finditer(r"^\|\s*([^|]+?)\s*\|", block, re.M) if not set(m.group(1).strip()) <= set("-: ") and m.group(1).strip() not in ("Risk", "Defend row")]
    dm = re.search(r"\*\*Defend in v1\*\*(.*?)\*\*Accepted for v1\*\*", t16, re.S)
    tm = re.search(r"\*\*Where each Defend row is tested\.\*\*(.*?)^## ", t16, re.S | re.M)
    if dm and tm:
        tested = {}
        for m in re.finditer(r"^\|\s*([^|]+?)\s*\|\s*(.*?)\s*\|\s*$", tm.group(1), re.M):
            if m.group(1) not in ("Defend row",) and not set(m.group(1)) <= set("-: "):
                tested[m.group(1)] = m.group(2)
        for row in first_cells(dm.group(1)):
            if row not in tested:
                print(f"{adr16}: Defend row {row!r} has no row in the test table"); bad += 1
        try:
            code = subprocess.run(["git", "ls-files", "--", ".", ":!*.md"], capture_output=True, text=True, check=True).stdout.split()
        except (OSError, subprocess.CalledProcessError):
            code = []
        blob = ""
        for f in code:
            try:
                blob += open(f, encoding="utf-8", errors="ignore").read() + "\n"
            except OSError:
                pass
        for row, cell in tested.items():
            names = re.findall(r"`([^`]+)`", cell)
            if not names and "pending" not in cell:
                print(f"{adr16}: Defend row {row!r} names no test and is not pending"); bad += 1
            for name in names:
                if not re.search(r"\b" + re.escape(name) + r"\b", blob):
                    print(f"{adr16}: Defend row {row!r} names test {name!r}, which does not exist"); bad += 1
    else:
        print(f"{adr16}: cannot find the Defend table or the test table"); bad += 1

# check-trivyignore-dates (ADR 0017, D30): every entry of Trivy's ignore file has an ID, a statement and an expiry date, so no ignore is
# permanent. The file is read line by line: an entry starts with "- id:" (a full YAML parser is not installed everywhere).
try:
    lines = open(".trivyignore.yaml", encoding="utf-8").read().splitlines()
except OSError:
    lines = None
    print(".trivyignore.yaml: missing"); bad += 1
if lines is not None:
    entries, cur = [], None
    for raw in lines:
        line = raw.split("#", 1)[0].rstrip() if not raw.lstrip().startswith("- ") else raw.rstrip()
        m = re.match(r"^\s*-\s+(\w+):\s*(.*)$", line)
        if m and m.group(1) == "id":
            cur = {"id": m.group(2).strip()}
            entries.append(cur)
        elif m and cur is not None:
            cur[m.group(1)] = m.group(2).strip()
        else:
            k = re.match(r"^\s+(\w+):\s*(.*)$", line)
            if k and cur is not None:
                cur[k.group(1)] = k.group(2).strip()
    for e in entries:
        if not e.get("id"):
            print(".trivyignore.yaml: an entry has no id"); bad += 1
        if not e.get("statement"):
            print(f".trivyignore.yaml: {e.get('id')} has no statement (why it is ignored)"); bad += 1
        if not re.fullmatch(r"\d{4}-\d{2}-\d{2}", e.get("expired_at", "")):
            print(f".trivyignore.yaml: {e.get('id')} has no expired_at date (YYYY-MM-DD)"); bad += 1

print("docs ok" if not bad else f"{bad} problem(s)")
sys.exit(1 if bad else 0)
PY
