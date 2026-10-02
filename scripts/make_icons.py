# Generate the XenTerm icon set and the branded default wallpaper.
#
# Inputs:  the logo PNG (XenTerm "XT" mark, any square size >= 512).
# Outputs: assets/icon.png (256), assets/icon@512.png (512),
#          assets/xenterm.ico (multi-size), assets/xt.jpg (2560x1440 wallpaper).
#
# Usage: python tools/make_icons.py <logo.png>

import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "assets"
FONT = ROOT / "ui" / "fonts" / "MeatshellMono-Regular.ttf"

ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]

TEAL = (61, 220, 160)
WHITE = (235, 235, 235)
GREY = (120, 120, 124)
DARK = (17, 17, 20)


def trim_and_fill(img: Image.Image, fill: float = 0.94) -> Image.Image:
    """Crop low-alpha padding, then scale the emblem to `fill` of the canvas.

    The source artwork ships with a soft near-transparent fringe around the
    emblem (the solid part was only ~62% of the canvas), which made small
    sizes render the mark much smaller than icons drawn edge to edge. The
    threshold cut keeps the outer glow; `fill` leaves a little breathing room
    so rounded corners are not clipped at 16px.
    """
    alpha = img.getchannel("A")
    bbox = alpha.point(lambda a: 255 if a > 64 else 0).getbbox()
    if bbox:
        img = img.crop(bbox)
    side = max(img.size)
    canvas = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    canvas.paste(img, ((side - img.width) // 2, (side - img.height) // 2))
    target = round(side * fill / 2) * 2
    return canvas.resize((target, target), Image.LANCZOS)


def make_icons(logo: Path) -> None:
    img = trim_and_fill(Image.open(logo).convert("RGBA"))

    img.resize((512, 512), Image.LANCZOS).save(ASSETS / "icon@512.png")
    img.resize((256, 256), Image.LANCZOS).save(ASSETS / "icon.png")
    img.resize((256, 256), Image.LANCZOS).save(
        ASSETS / "xenterm.ico", format="ICO",
        sizes=[(s, s) for s in ICO_SIZES],
    )
    print("OK: icon.png, icon@512.png, xenterm.ico written")


def wallpaper() -> None:
    w, h = 2560, 1440
    img = Image.new("RGB", (w, h), DARK)
    draw = ImageDraw.Draw(img, "RGBA")

    # Vertical gradient, slightly lighter at the top.
    for y in range(h):
        t = y / h
        base = tuple(int(23 + (11 - 23) * t + i) for i in (0, 0, 0))
        draw.line([(0, y), (w, y)], fill=base)

    # Dot-matrix field, fading out from the centre — the old wallpaper's map
    # dots, abstracted.
    step = 26
    for y in range(120, h - 260, step):
        for x in range(90, w - 90, step):
            dx, dy = (x - w / 2) / (w / 2), (y - h / 2) / (h / 2)
            fade = max(0.0, 1.0 - (dx * dx + dy * dy) * 0.85)
            if fade <= 0.05:
                continue
            alpha = int(26 * fade)
            draw.ellipse((x, y, x + 2, y + 2), fill=(200, 200, 205, alpha))

    # Faint wave ridges along the bottom, in the brand teal.
    import math
    for layer in range(7):
        amp = 46 + layer * 18
        base_y = h - 240 + layer * 30
        freq = 1.6 + layer * 0.35
        phase = layer * 1.9
        colour = (24 + layer * 4, 70 + layer * 9, 55 + layer * 8, 60)
        pts = [(x, base_y - amp * (0.55 + 0.45 * math.sin(x / w * freq * 6.283 + phase)))
               for x in range(0, w + 8, 8)]
        draw.line(pts, fill=colour, width=2)

    mono_l = ImageFont.truetype(str(FONT), 66)
    mono_s = ImageFont.truetype(str(FONT), 30)
    mono_n = ImageFont.truetype(str(FONT), 22)

    # Center wordmark: ">_ XenTerm" with the brand split across the name.
    cx, cy = w / 2, h * 0.44
    prompt_w = draw.textlength(">_ ", font=mono_l)
    xen_w = draw.textlength("Xen", font=mono_l)
    term_w = draw.textlength("Term", font=mono_l)
    total = prompt_w + xen_w + term_w
    x0 = cx - total / 2
    draw.text((x0, cy), ">_ ", font=mono_l, fill=WHITE)
    draw.text((x0 + prompt_w, cy), "Xen", font=mono_l, fill=TEAL)
    draw.text((x0 + prompt_w + xen_w, cy), "Term", font=mono_l, fill=WHITE)

    sub = "SSH Terminal · SFTP · System Monitor"
    draw.text((cx - draw.textlength(sub, font=mono_s) / 2, cy + 96),
              sub, font=mono_s, fill=GREY)

    # Top-right neofetch-style block, like the old default wallpaper.
    rows = [("OS", "Linux"), ("Host", "Remote Server"), ("Uptime", "12d 6h 34m"),
            ("Users", "root"), ("Shell", "xenterm"), ("Theme", "dark")]
    tx, ty = w - 420, 56
    for i, (key, value) in enumerate(rows):
        yy = ty + i * 34
        draw.text((tx, yy), f"{key}: ", font=mono_n, fill=GREY)
        draw.text((tx + draw.textlength(f"{key}: ", font=mono_n), yy),
                  value, font=mono_n, fill=WHITE)
    draw.text((tx, ty + len(rows) * 34), ">_", font=mono_n, fill=GREY)

    img.save(ASSETS / "xt.jpg", quality=92)
    print("OK: xt.jpg written")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: python tools/make_icons.py <logo.png>")
    logo = Path(sys.argv[1])
    make_icons(logo)
    wallpaper()
