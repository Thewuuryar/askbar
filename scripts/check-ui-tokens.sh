#!/bin/sh
# Fails if the admin copy of tokens.css differs from clients/desktop/ui/tokens.css, or if --fg, --muted, --accent, --danger or --good is under
# 4.5:1 contrast against --bg or --card, --accent-fg under 4.5:1 against --accent, or --fg under 4.5:1 on
# --danger-soft (a failed question's tint) laid over --card, --fg or --muted under 4.5:1 on --surface-2, an icon colour
# (--fg, --muted, --accent, --danger, --good) under 3:1 on --bg, --card or --surface-2, or --field-border under 3:1
# against --bg or --card, in either theme (rule 43).
# Also fails if services/admin/src/ui/style.css sets a font-size other than one of the four --text-* tokens (or inherit),
# a font-weight other than --weight-regular, --weight-bold or inherit, or a font: shorthand with a px, rem, em or pt size.
# Usage: scripts/check-ui-tokens.sh
cd "$(dirname "$0")/.." || exit 2
python3 - <<'PY'
import re, sys
a = open("clients/desktop/ui/tokens.css", encoding="utf-8").read()
if a != open("services/admin/src/ui/tokens.css", encoding="utf-8").read():
    print("services/admin/src/ui/tokens.css differs from clients/desktop/ui/tokens.css; copy it"); sys.exit(1)

def block(start):
    i = a.index(start); j = a.index("}", i)
    return dict(re.findall(r"--([\w-]+):\s*(#[0-9a-fA-F]{6}|rgba\([^)]*\))\s*;", a[i:j]))

def over(rgba, under):
    # An rgba() colour laid over an opaque #rrggbb one, as #rrggbb.
    r, g, b, alpha = [float(x) for x in rgba[5:-1].split(",")]
    u = [int(under[i:i + 2], 16) for i in (1, 3, 5)]
    return "#" + "".join(f"{round(c * alpha + d * (1 - alpha)):02x}" for c, d in zip((r, g, b), u))

def lum(h):
    c = [int(h[i:i + 2], 16) / 255 for i in (1, 3, 5)]
    c = [x / 12.92 if x <= 0.03928 else ((x + 0.055) / 1.055) ** 2.4 for x in c]
    return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]

def ratio(x, y):
    hi, lo = sorted((lum(x), lum(y)), reverse=True)
    return (hi + 0.05) / (lo + 0.05)

bad = 0
for theme, t in (("light", block(":root {")), ("dark", block(':root[data-theme="dark"] {'))):
    for fg in ("fg", "muted", "accent", "danger", "good"):
        for bg in ("bg", "card"):
            r = ratio(t[fg], t[bg])
            if r < 4.5:
                print(f"{theme}: --{fg} on --{bg} is {r:.2f}:1 (needs 4.5)"); bad += 1
    # Table headers and the hover fill sit on --surface-2: text there needs 4.5:1 too.
    for fg in ("fg", "muted"):
        r = ratio(t[fg], t["surface-2"])
        if r < 4.5:
            print(f"{theme}: --{fg} on --surface-2 is {r:.2f}:1 (needs 4.5)"); bad += 1
    # Icons (drawn in currentColor) need 3:1 on every surface (rule 43).
    for fg in ("fg", "muted", "accent", "danger", "good"):
        for bg in ("bg", "card", "surface-2"):
            r = ratio(t[fg], t[bg])
            if r < 3:
                print(f"{theme}: an icon in --{fg} on --{bg} is {r:.2f}:1 (needs 3)"); bad += 1
    # A field's border against the field (--bg) and what it sits on (--bg, --card) needs 3:1 (rule 43).
    for bg in ("bg", "card"):
        r = ratio(t["field-border"], t[bg])
        if r < 3:
            print(f"{theme}: --field-border on --{bg} is {r:.2f}:1 (needs 3)"); bad += 1
    r = ratio(t["accent-fg"], t["accent"])
    if r < 4.5:
        print(f"{theme}: --accent-fg on --accent is {r:.2f}:1 (needs 4.5)"); bad += 1
    r = ratio(t["fg"], over(t["danger-soft"], t["card"]))
    if r < 4.5:
        print(f"{theme}: --fg on --danger-soft over --card is {r:.2f}:1 (needs 4.5)"); bad += 1
# Type scale (rules 14, 15): the admin stylesheet uses the four --text-* sizes and the two weights only.
SIZES = {"var(--text-xs)", "var(--text-s)", "var(--text-m)", "var(--text-l)", "inherit"}
WEIGHTS = {"var(--weight-regular)", "var(--weight-bold)", "inherit"}
css = open("services/admin/src/ui/style.css", encoding="utf-8").read()
for n, line in enumerate(css.splitlines(), 1):
    problems = []
    for v in re.findall(r"(?<![\w-])font-size\s*:\s*([^;}]+)", line):
        if v.strip() not in SIZES:
            problems.append(f"font-size: {v.strip()} is not a --text-* token")
    for v in re.findall(r"(?<![\w-])font-weight\s*:\s*([^;}]+)", line):
        if v.strip() not in WEIGHTS:
            problems.append(f"font-weight: {v.strip()} is not --weight-regular or --weight-bold")
    for v in re.findall(r"(?<![\w-])font\s*:\s*([^;}]+)", line):
        if re.search(r"\d\s*(px|rem|em|pt)\b", v):
            problems.append(f"font shorthand sets a literal size: {v.strip()}")
    for p in problems:
        print(f"style.css line {n}: {p}\n    {line.strip()}"); bad += 1
print("ui tokens ok" if not bad else f"{bad} problem(s)")
sys.exit(1 if bad else 0)
PY
