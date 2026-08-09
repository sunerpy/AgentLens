#!/usr/bin/env python3
"""Parametric generator for the AgentLens hexagon glyph.

Design contract (see report):
  - ONE pointy-top regular hexagon, no background plate, fully transparent canvas.
  - Three facets: left half = light, right-upper = orange, right-lower = teal.
  - A mid-slate silhouette stroke (#64748B) carries the shape on LIGHT backgrounds,
    where the light left facet has only 1.02:1 contrast against #F3F3F3 and would
    otherwise vanish. Slate-500 is the only value measured above 2.5:1 against BOTH
    #FFFFFF (4.76) and #202020 (3.42).
  - Every size is rendered NATIVELY from size-tuned geometry, never downsampled,
    with the two vertical edges snapped to integer pixel boundaries.
"""

import math
import os
import shutil

# ---------------------------------------------------------------- design tokens
FACE_LIGHT = "#E7EDF5"  # lit facet   2.38:1 vs orange -> still reads as the light face
FACE_ORANGE = "#F97316"  # orange-500  2.80:1 on white
FACE_TEAL = "#14B8A6"  # teal-500    2.49:1 on white
STROKE = "#64748B"  # slate-500   4.76:1 on white / 3.42:1 on #202020

FILL_FRACTION = 0.96  # glyph height (stroke included) as a fraction of the canvas
STROKE_RATIO = 0.028  # stroke width as a fraction of the canvas, min 1 device px

SQRT3_2 = math.sqrt(3) / 2.0


def geometry(size: int, snap: bool):
    """Return (cx, cy, halfwidth, R, stroke_w) for a pointy-top hexagon.

    halfwidth = R * sqrt(3)/2 is the distance from the centre to the vertical edges.
    When `snap` is set, halfwidth is rounded so those two vertical edges land exactly
    on integer pixel boundaries -- the single biggest win for small-size crispness,
    since the diagonals get antialiased no matter what.
    """
    stroke_w = max(1.0, size * STROKE_RATIO)
    r = (FILL_FRACTION * size - stroke_w) / 2.0
    halfwidth = r * SQRT3_2
    if snap:
        halfwidth = round(halfwidth)
        r = halfwidth / SQRT3_2
    return size / 2.0, size / 2.0, halfwidth, r, stroke_w


def fmt(v: float) -> str:
    s = f"{v:.3f}".rstrip("0").rstrip(".")
    return s if s not in ("", "-0") else "0"


def paths(size: int, snap: bool, seams: bool):
    cx, cy, hw, r, sw = geometry(size, snap)
    top, bot = cy - r, cy + r
    sh_up, sh_dn = cy - r / 2.0, cy + r / 2.0  # shoulder ys (the 4 side vertices)
    left, right = cx - hw, cx + hw

    def p(pts):
        return "M " + " L ".join(f"{fmt(x)} {fmt(y)}" for x, y in pts) + " Z"

    hexagon = p(
        [
            (cx, top),
            (right, sh_up),
            (right, sh_dn),
            (cx, bot),
            (left, sh_dn),
            (left, sh_up),
        ]
    )
    face_l = p([(cx, top), (left, sh_up), (left, sh_dn), (cx, bot)])
    face_o = p([(cx, top), (right, sh_up), (right, cy), (cx, cy)])
    face_t = p([(cx, cy), (right, cy), (right, sh_dn), (cx, bot)])

    seam_v = f"M {fmt(cx)} {fmt(top)} L {fmt(cx)} {fmt(bot)}" if seams else None
    seam_h = f"M {fmt(cx)} {fmt(cy)} L {fmt(right)} {fmt(cy)}" if seams else None
    return hexagon, face_l, face_o, face_t, seam_v, seam_h, sw


def svg(size: int, snap: bool = False, seams: bool = True, commented: bool = False) -> str:
    hexagon, face_l, face_o, face_t, seam_v, seam_h, sw = paths(size, snap, seams)
    j = "stroke-linejoin=\"round\" stroke-linecap=\"round\""
    c = (lambda t: f"  <!-- {t} -->\n") if commented else (lambda t: "")
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" '
        f'viewBox="0 0 {size} {size}" fill="none">\n'
    ]
    if commented:
        out.append(
            "  <!-- AgentLens glyph. Pointy-top hexagon, three facets, no plate,\n"
            "       transparent canvas. The slate-500 outline is what keeps the shape\n"
            "       legible on light backgrounds, where the light facet is ~1.0:1. -->\n"
        )
    out.append(c("facets"))
    out.append(f'  <path d="{face_l}" fill="{FACE_LIGHT}"/>\n')
    out.append(f'  <path d="{face_o}" fill="{FACE_ORANGE}"/>\n')
    out.append(f'  <path d="{face_t}" fill="{FACE_TEAL}"/>\n')
    if seams:
        out.append(c("facet seams"))
        out.append(
            f'  <path d="{seam_v}" stroke="{STROKE}" stroke-width="{fmt(sw)}" {j}/>\n'
        )
        out.append(
            f'  <path d="{seam_h}" stroke="{STROKE}" stroke-width="{fmt(sw)}" {j}/>\n'
        )
    out.append(c("silhouette -- carries the shape on light backgrounds"))
    out.append(f'  <path d="{hexagon}" stroke="{STROKE}" stroke-width="{fmt(sw)}" {j}/>\n')
    out.append("</svg>\n")
    return "".join(out)


# Below this size the two facet seams close up against the silhouette and turn the
# glyph into mush; verified by eye on native 16/24/32 renders. Above it they read.
SEAM_FLOOR = 32

FLAT_PNGS = {
    "32x32.png": 32,
    "64x64.png": 64,
    "128x128.png": 128,
    "128x128@2x.png": 256,
    "icon.png": 512,
    "Square30x30Logo.png": 30,
    "Square44x44Logo.png": 44,
    "Square71x71Logo.png": 71,
    "Square89x89Logo.png": 89,
    "Square107x107Logo.png": 107,
    "Square142x142Logo.png": 142,
    "Square150x150Logo.png": 150,
    "Square284x284Logo.png": 284,
    "Square310x310Logo.png": 310,
    "StoreLogo.png": 50,
}

# 32 MUST come first: Tauri's dev mode reads layer[0], and a wrong first layer makes
# the dev icon look broken while every other surface looks fine.
ICO_SIZES = [32, 16, 24, 48, 64, 256]


def render(size: int):
    import io

    import cairosvg
    from PIL import Image

    png = cairosvg.svg2png(
        bytestring=svg(size, snap=True, seams=(size >= SEAM_FLOOR)).encode(),
        output_width=size,
        output_height=size,
    )
    return Image.open(io.BytesIO(png)).convert("RGBA")


def write_ico(path: str) -> None:
    """Hand-built ICO so the directory order is exactly ICO_SIZES.

    Pillow's ICO writer sorts entries by size, which would bury the 32px layer.
    """
    import io
    import struct

    blobs = []
    for s in ICO_SIZES:
        buf = io.BytesIO()
        render(s).save(buf, format="PNG", optimize=True)
        blobs.append(buf.getvalue())

    out = bytearray(struct.pack("<HHH", 0, 1, len(ICO_SIZES)))
    offset = 6 + 16 * len(ICO_SIZES)
    for s, blob in zip(ICO_SIZES, blobs):
        dim = 0 if s == 256 else s  # 0 encodes 256 in the ICO directory
        out += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(blob), offset)
        offset += len(blob)
    for blob in blobs:
        out += blob
    open(path, "wb").write(bytes(out))


if __name__ == "__main__":
    import subprocess
    import sys

    import cairosvg

    here = os.path.dirname(os.path.abspath(__file__))
    icons = os.path.dirname(here)

    with open(f"{here}/icon.svg", "w") as fh:
        fh.write(svg(1024, snap=False, seams=True, commented=True))
    cairosvg.svg2png(
        bytestring=svg(1024, snap=False, seams=True).encode(),
        write_to=f"{here}/icon-1024.png",
        output_width=1024,
        output_height=1024,
    )
    print("wrote source/icon.svg + source/icon-1024.png")

    if "--skip-tauri" not in sys.argv:
        subprocess.run(
            ["cargo", "tauri", "icon", f"{here}/icon-1024.png", "-o", icons],
            check=True,
            cwd=os.path.dirname(icons),
        )
        # tauri.conf.json targets only deb + nsis; mobile icon trees are dead weight
        for d in ("android", "ios"):
            shutil.rmtree(f"{icons}/{d}", ignore_errors=True)
        print("ran cargo tauri icon; removed android/ + ios/")

    for name, size in FLAT_PNGS.items():
        img = render(size)
        assert img.mode == "RGBA" and img.size == (size, size), (name, img.mode, img.size)
        img.save(f"{icons}/{name}")
    print(f"re-rendered {len(FLAT_PNGS)} PNGs natively")

    write_ico(f"{icons}/icon.ico")
    print("wrote icon.ico", ICO_SIZES)
