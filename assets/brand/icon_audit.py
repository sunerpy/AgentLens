#!/usr/bin/env python3
"""Quantitative acceptance audit for AgentLens app-icon candidates.

Why this exists
---------------
The agent driving this repo cannot look at images, so "does the icon read at
16x16?" has to become arithmetic. This script rasterises each candidate SVG at
1024x1024 with cairosvg, composites it the way an OS shell would (over a dark
shell and over a light shell), downsamples to the worst realistic case -- the
16x16 Windows taskbar slot -- and scores five properties with hard thresholds.

Metrics and thresholds
----------------------
1. foreground_fraction   0.35 <= f <= 0.70
     Share of the 16x16 tile classified as accent/foreground. Too low means the
     mark is too thin to see; too high means it fills into an undifferentiated
     blob.
2. stroke_p10_px         >= 1.5
     Run-length scan of every row and column of the 16x16 foreground mask. The
     literal minimum run is reported too, but it is degenerate for curved forms
     (a single tangent pixel makes it 1 for any circle), so the gate uses the
     10th percentile of run lengths as the robust "thinnest real stroke".
3. sig_colors            <= 16
     Distinct colours in the 16x16 tile after 4-bit-per-channel quantisation,
     counting only colours covering >= 1% (>= 3 px). Gradients and antialiasing
     mush inflate this.
4. contrast_dark / contrast_light   both >= 0.25
     |mean luminance(foreground) - mean luminance(background)| on the 16x16
     tile, composited over #0B0D10 and over #FFFFFF. Relative luminance uses
     sRGB coefficients without gamma linearisation (monotonic, sufficient for a
     threshold test).
5. shape_iou             >= 0.60
     Downsample the 1024 foreground mask to 16x16, re-binarise, nearest-upscale
     back to 1024, and intersect-over-union against the original mask. Measures
     how much of the drawn form actually survives the trip to 16px.

Usage:  python3 assets/brand/icon_audit.py [svg ...]
Exit code is 0 only when at least one candidate passes every gate.
"""

from __future__ import annotations

import io
import json
import sys
from pathlib import Path

import cairosvg
import numpy as np
from PIL import Image

MASTER = 1024
SMALL = 16
DARK_BG = (0x0B, 0x0D, 0x10)
LIGHT_BG = (0xFF, 0xFF, 0xFF)

THRESHOLDS = {
    "foreground_fraction": (0.35, 0.70),
    "stroke_p10_px": (1.5, None),
    "sig_colors": (None, 16),
    "contrast_dark": (0.25, None),
    "contrast_light": (0.25, None),
    "shape_iou": (0.60, None),
}


def render_master(svg_path: Path) -> Image.Image:
    """Rasterise an SVG to a 1024x1024 RGBA image (deterministic for a given input)."""
    png = cairosvg.svg2png(
        url=str(svg_path), output_width=MASTER, output_height=MASTER
    )
    return Image.open(io.BytesIO(png)).convert("RGBA")


def composite(img: Image.Image, bg: tuple[int, int, int]) -> Image.Image:
    plate = Image.new("RGBA", img.size, bg + (255,))
    return Image.alpha_composite(plate, img).convert("RGB")


def luminance(rgb: np.ndarray) -> np.ndarray:
    f = rgb.astype(np.float64) / 255.0
    return 0.2126 * f[..., 0] + 0.7152 * f[..., 1] + 0.0722 * f[..., 2]


def otsu(lum: np.ndarray) -> float:
    """Self-calibrating foreground/background split; no hard-coded brand colours."""
    hist, edges = np.histogram(lum, bins=64, range=(0.0, 1.0))
    total = hist.sum()
    if total == 0:
        return 0.5
    centers = (edges[:-1] + edges[1:]) / 2.0
    omega = np.cumsum(hist) / total
    mu = np.cumsum(hist * centers) / total
    mu_t = mu[-1]
    denom = omega * (1.0 - omega)
    with np.errstate(divide="ignore", invalid="ignore"):
        sigma_b = np.where(denom > 0, (mu_t * omega - mu) ** 2 / denom, 0.0)
    return float(centers[int(np.nanargmax(sigma_b))])


def runs(mask: np.ndarray) -> list[int]:
    """Lengths of every horizontal and vertical maximal foreground run."""
    out: list[int] = []
    for line in list(mask) + list(mask.T):
        n = 0
        for v in line:
            if v:
                n += 1
            elif n:
                out.append(n)
                n = 0
        if n:
            out.append(n)
    return out


def sig_colors(rgb: np.ndarray, min_share: float = 0.01) -> int:
    q = (rgb >> 4).reshape(-1, 3)
    _, counts = np.unique(q, axis=0, return_counts=True)
    return int((counts >= max(1, int(round(min_share * q.shape[0])))).sum())


def contrast(rgb: np.ndarray, mask: np.ndarray) -> float:
    lum = luminance(rgb)
    if mask.all() or not mask.any():
        return 0.0
    return float(abs(lum[mask].mean() - lum[~mask].mean()))


def audit(svg_path: Path) -> dict:
    master = render_master(svg_path)

    dark_master = np.asarray(composite(master, DARK_BG))
    thr_master = otsu(luminance(dark_master))
    mask_master = luminance(dark_master) > thr_master

    # Composite first, then downsample: this is the order an OS shell uses.
    dark_small = np.asarray(
        composite(master, DARK_BG).resize((SMALL, SMALL), Image.LANCZOS)
    )
    light_small = np.asarray(
        composite(master, LIGHT_BG).resize((SMALL, SMALL), Image.LANCZOS)
    )
    lum_small = luminance(dark_small)
    mask_small = lum_small > otsu(lum_small)

    run_lengths = runs(mask_small)
    # shape retention: 1024 mask -> 16 -> re-binarise -> nearest back to 1024
    small_of_mask = np.asarray(
        Image.fromarray((mask_master * 255).astype(np.uint8)).resize(
            (SMALL, SMALL), Image.LANCZOS
        )
    )
    rebin = small_of_mask > 127
    up = np.asarray(
        Image.fromarray((rebin * 255).astype(np.uint8)).resize(
            (MASTER, MASTER), Image.NEAREST
        )
    ) > 127
    inter = np.logical_and(mask_master, up).sum()
    union = np.logical_or(mask_master, up).sum()

    m = {
        "foreground_fraction": float(mask_small.mean()),
        "stroke_min_px": float(min(run_lengths)) if run_lengths else 0.0,
        "stroke_p10_px": float(np.percentile(run_lengths, 10)) if run_lengths else 0.0,
        "sig_colors": sig_colors(dark_small),
        "contrast_dark": contrast(dark_small, mask_small),
        "contrast_light": contrast(light_small, mask_small),
        "shape_iou": float(inter / union) if union else 0.0,
    }

    violations = []
    for key, (lo, hi) in THRESHOLDS.items():
        v = m[key]
        if lo is not None and v < lo:
            violations.append(f"{key}={v:.3f} < {lo}")
        if hi is not None and v > hi:
            violations.append(f"{key}={v:.3f} > {hi}")
    return {"name": svg_path.stem, "metrics": m, "violations": violations,
            "verdict": "PASS" if not violations else "FAIL"}


def main(argv: list[str]) -> int:
    paths = [Path(a) for a in argv[1:]]
    if not paths:
        paths = sorted(Path(__file__).parent.glob("candidate-*.svg"))
    results = [audit(p) for p in paths]

    cols = ["foreground_fraction", "stroke_min_px", "stroke_p10_px", "sig_colors",
            "contrast_dark", "contrast_light", "shape_iou"]
    head = ["candidate"] + cols + ["verdict"]
    print("| " + " | ".join(head) + " |")
    print("|" + "|".join(["---"] * len(head)) + "|")
    for r in results:
        cells = [r["name"]]
        for c in cols:
            v = r["metrics"][c]
            cells.append(f"{v:d}" if isinstance(v, int) else f"{v:.3f}")
        cells.append(r["verdict"])
        print("| " + " | ".join(cells) + " |")
    print()
    print("gates: " + json.dumps(THRESHOLDS))
    for r in results:
        if r["violations"]:
            print(f"{r['name']}: REJECTED -> " + "; ".join(r["violations"]))
        else:
            print(f"{r['name']}: all gates satisfied")
    return 0 if any(r["verdict"] == "PASS" for r in results) else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
