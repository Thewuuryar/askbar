#!/bin/sh
# Fails if the admin copy of tokens.css differs from ui/tokens.css, or if --fg, --muted, --accent, --danger or --good is under
# 4.5:1 contrast against --bg or --card, or --accent-fg under 4.5:1 against --accent, in either theme.
# Usage: scripts/check-ui-tokens.sh
cd "$(dirname "$0")/.." || exit 2
python3 - <<'PY'
import re, sys
a = open("ui/tokens.css", encoding="utf-8").read()
if a != open("services/admin/src/ui/tokens.css", encoding="utf-8").read():
    print("services/admin/src/ui/tokens.css differs from ui/tokens.css; copy it"); sys.exit(1)

def block(start):
    i = a.index(start); j = a.index("}", i)
    return dict(re.findall(r"--([\w-]+):\s*(#[0-9a-fA-F]{6})\s*;", a[i:j]))

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
    r = ratio(t["accent-fg"], t["accent"])
    if r < 4.5:
        print(f"{theme}: --accent-fg on --accent is {r:.2f}:1 (needs 4.5)"); bad += 1
print("ui tokens ok" if not bad else f"{bad} problem(s)")
sys.exit(1 if bad else 0)
PY
