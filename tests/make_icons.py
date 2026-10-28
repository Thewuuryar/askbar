"""Generate Askbar's app icons (run from the repo root: python3 tests/make_icons.py).

A rounded cobalt square with a white input-bar pill and a caret.
"""
from PIL import Image, ImageDraw

COBALT = (62, 99, 221, 255)
INK = (22, 24, 29, 255)
WHITE = (255, 255, 255, 255)
S = 1024


def draw(size: int) -> Image.Image:
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle((48, 48, S - 48, S - 48), radius=220, fill=COBALT)
    # the bar
    d.rounded_rectangle((170, 392, S - 170, 632), radius=120, fill=WHITE)
    # the caret
    d.rounded_rectangle((258, 450, 290, 574), radius=16, fill=COBALT)
    # a few "typed" dashes
    d.rounded_rectangle((350, 497, 640, 527), radius=15, fill=INK[:3] + (70,))
    return img.resize((size, size), Image.LANCZOS)


out = "src-tauri/icons/"
for name, size in [("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256), ("icon.png", 512)]:
    draw(size).save(out + name)
draw(256).save(out + "icon.ico", sizes=[(16, 16), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
draw(512).save(out + "icon.icns")
print("icons written")
