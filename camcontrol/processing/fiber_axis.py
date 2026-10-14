"""Axis of symmetry of polarization-maintaining (PM) fiber end faces.

PM fibers have stress parts on either side of the core: two round rods
(Panda), two wedges (Bow-tie), or an elliptical region round the core
(Elliptical-clad). The line through them is the fiber's slow axis; the
fast axis is at 90 degrees. Both are mirror-symmetry axes of the end face.

How it's measured, for each fiber:
1. Find the fiber: the round cladding, or a circle given by the caller
   (e.g. a Circle measurement). Separate fibers are found as round bright
   (or dark) regions. Fibers touching each other (e.g. side by side in a
   V-groove) or running off the image are found by find_touching_fibers:
   the biggest one first, then the others as circles of the same size.
2. Find the stress parts: inside the cladding, the pixels clearly different
   from the cladding (see contrast_map). For each Lab channel a smooth
   surface (tilt plus round bowl, fitted to the cladding only) is taken off, so uneven
   lighting doesn't count, and the rest is divided by the cladding's noise
   level. So a faint brightness-only difference counts as much as a strong
   color one. The contrast is in noise units; the threshold is automatic
   (Otsu) or set by the user (Detection). The core, in the middle, and the
   edge of the cladding are left out.
3. The slow axis through them, by one of these methods (METHODS):
   - "auto": "rods" for Panda fibers, "symmetry" for Bow-tie and
     Elliptical ones, chosen by which outline fits the edges best (see
     shape_fits and _measure_auto). The default.
   - "rods": for Panda fibers. Looks for the outlines of two equal round
     rods placed either side of the fiber centre (see find_rods), using
     only edges, so shading and dark blobs inside the fiber don't count.
     Each rod is then fitted on its own; the axis is the line through
     their centres. Needs no threshold. The result is unclear if the rod
     outlines are faint or not round (quality, coverage), e.g. on a Bow-tie
     fiber: use "symmetry" for those.
   - "symmetry": the line the end face is most mirror-symmetric about
     (with the line at 90 degrees, as both are mirror axes). Uses the
     contrast map directly, so no threshold, and lopsided stress parts
     don't pull it.
   - "centres": the line through the centres of the two stress parts
     (Panda, Bow-tie). Each part counts the same whatever its shape.
     A single part (Elliptical) falls back to "moments".
   - "moments": the principal axis (second moments about the fiber centre)
     of all stress-part pixels. Pixels count by distance squared, so the
     far corners of bow-tie wedges weigh most.
   "symmetry" and "moments" turn about the fiber centre, or with
   parts_centre=True about the centre of the stress parts (for stress parts
   that sit off the fiber's middle); the axis lines are drawn through it.
   How much longer the spread is along the axis than across it
   ("elongation", from the moments) says how clear the result is; near 1
   means no clear axis.
4. Type guess from the stress parts' shape: one region round the core
   (Elliptical), two round blobs (Panda), two other shapes (Bow-tie).

Angles are in degrees, counter-clockwise from horizontal as seen on screen
(image y points down, so it's flipped), from 0 to 180.

Run directly to test on an image (default: "Fiber Types.png"):
    python -m camcontrol.processing.fiber_axis [image]
"""

import math
import sys
from dataclasses import dataclass, field
from pathlib import Path

import cv2
import numpy as np

from camcontrol.processing.common import as_bgr

CORE_FRACTION = 0.12   # inner part of the radius left out (the core)
EDGE_FRACTION = 0.92   # outer part left out (the cladding edge)
MIN_ELONGATION = 1.3   # below this the axis isn't clear
SYMMETRY_SIZE = 200    # radius (px) the fiber is scaled to for the symmetry search
NOISE_FLOOR = 0.5      # Lab units: smallest noise level assumed (clean or drawn images)
MIN_AUTO_THRESHOLD = 2.5  # noise units: the automatic threshold never goes lower (no real contrast)
FIT_SAMPLES = 40000    # about this many pixels are used to fit the cladding surface
ROD_SEARCH_EDGE = 0.88  # rod search: edges beyond this fraction of the radius are left out (cladding edge, dirt)
ROD_MAX_REACH = 0.85    # rods must lie inside this fraction of the radius
ROD_SIZES = (0.18, 0.42)     # rod radius searched, fraction of the fiber radius (Panda rods: about 0.25-0.3)
ROD_SPACINGS = (0.30, 0.72)  # rod centre's distance from the core searched, fraction of the fiber radius
EDGE_CAP = 4.0               # rod search: an edge counts at most this many times the typical gradient
GRADIENT_FLOOR = 0.5         # rod search: smallest typical gradient assumed (grey levels per px)
ROD_STARTS = 5               # rod search: best different pairs refined (step 2 of find_rods)
MATCH_SCORE = 0.8            # rods matched to each other (_match_rods): smallest match score used
MATCH_AGREE = 3.0            # ... and largest difference (degrees) from the edge fits' line
MIN_ROD_QUALITY = 2.0   # rod pair's score / a typical pair's: below this the rods aren't clear
ON_CIRCLE_SHARE = 0.6   # see _edge_coverage
MIN_ROD_COVERAGE = 0.5  # part of each rod's outline with an edge right on it (clean rods ~0.8-1, dirty ~0.5-0.8)
SURE_PANDA = (3.0, 0.8)  # quality, coverage: plainly round rods, no need to try other shapes
OTHER_SHAPE_MARGIN = 0.0   # "auto": a wedge or ellipse must fit at least this much better than the rods to win
TURNED_GAIN = 0.05         # "auto": ... and the two parts must match better turned 180 degrees than side by side
                           # (Panda rods: -0.1 to 0.01; wedges and ellipse ends: 0.09 and up)
OTHER_SHAPE_QUALITY = 2.4  # "auto": other shapes are only tried when the rod search found strong edges
                           # (wedges and ellipses give 2.7 and up; a failed search on a dirty Panda, about 2)

METHODS = {  # name: label in the panel
    "auto": "Automatic",
    "rods": "Panda rod outlines",
    "symmetry": "Mirror symmetry",
    "centres": "Stress part centres",
    "moments": "Second moments",
}

LOOK_AT = {  # name: label in the panel, Lab channels used
    "both": ("Brightness and color", (0, 1, 2)),
    "brightness": ("Brightness only", (0,)),
    "color": ("Color only", (1, 2)),
}


@dataclass
class Detection:
    """How stress parts are told apart from the cladding."""
    threshold: float | None = None  # contrast (noise units) a stress part must reach; None: automatic
    smoothing: float = 1.0          # blur, % of the fiber radius
    look_at: str = "both"           # see LOOK_AT
    min_size: float = 8.0           # smaller regions are specks: side of a square, % of the radius


@dataclass
class FiberAxis:
    cx: float
    cy: float
    radius: float
    slow_deg: float        # angle of the slow axis (through the stress parts)
    elongation: float      # spread along / across the axis; >= 1
    fiber_type: str        # "Panda", "Bow-tie", "Elliptical" or "?"
    n_parts: int           # stress regions found
    method: str = "moments"  # what the angle came from (see METHODS)
    axis_x: float | None = None  # point the axis goes through; None: the fiber centre
    axis_y: float | None = None
    parts: list[np.ndarray] = field(default_factory=list)  # stress part outlines, (n, 2) image pixels
    part_centres: list[tuple[float, float]] = field(default_factory=list)
    threshold: float = 0.0           # contrast threshold used (noise units)
    quality: float | None = None     # "rods": how clearly the rod outlines stand out (see MIN_ROD_QUALITY)
    coverage: float | None = None    # "rods": how much of the rod outlines was found (see MIN_ROD_COVERAGE)
    rod_radii: list[float] = field(default_factory=list)  # "rods": radius of each rod, as part_centres
    turned_gain: float | None = None  # "rods": see _match_rods; high means wedges or an ellipse, not rods

    @property
    def axis_point(self) -> tuple[float, float]:
        if self.axis_x is None:
            return self.cx, self.cy
        return self.axis_x, self.axis_y

    @property
    def fast_deg(self) -> float:
        return (self.slow_deg + 90) % 180

    @property
    def clear(self) -> bool:
        if self.quality is not None:
            return self.quality >= MIN_ROD_QUALITY and self.coverage >= MIN_ROD_COVERAGE
        return self.elongation >= MIN_ELONGATION


def find_fibers(image: np.ndarray, max_fibers: int = 10) -> list[tuple[float, float, float]]:
    """Round fibers in the image as (cx, cy, radius), the biggest first.

    Both finders run: round regions clearly brighter (or darker) than the
    background, and find_touching_fibers (fibers touching or running off the
    image). Every circle is then fitted to its cladding edge, and circles
    that repeat one already kept, or lie inside it (a rod, a dark patch),
    are dropped. If nothing is found, circle-edge detection (Hough) is used.
    """
    gray = cv2.cvtColor(as_bgr(image), cv2.COLOR_BGR2GRAY)
    found = []
    for c in find_touching_fibers(image, max_fibers) + _find_round_regions(image):
        fitted = fit_cladding_edge(gray, *c)
        if fitted is not None and abs(fitted[2] - c[2]) < 0.2 * c[2] and math.hypot(fitted[0] - c[0], fitted[1] - c[1]) < 0.2 * c[2]:
            c = fitted
        if all(math.hypot(c[0] - k[0], c[1] - k[1]) > 0.8 * max(k[2], c[2]) for k in found):
            found.append(c)
    if not found:
        found = _find_hough_circles(image)
    return sorted(found, key=lambda c: -c[2])[:max_fibers]


def nearest_centre(circles: list[tuple[float, float, float]], shape) -> list[tuple[float, float, float]]:
    """Just the circle whose centre is nearest the middle of an image of this shape."""
    if not circles:
        return []
    h, w = shape[:2]
    return [min(circles, key=lambda c: math.hypot(c[0] - w / 2, c[1] - h / 2))]


def _fill_holes(mask: np.ndarray) -> np.ndarray:
    """mask with every region not reachable from outside the image filled in."""
    h, w = mask.shape
    outside = cv2.copyMakeBorder(mask, 1, 1, 1, 1, cv2.BORDER_CONSTANT, value=0)
    cv2.floodFill(outside, np.zeros((h + 4, w + 4), np.uint8), (0, 0), 255)
    return mask | (255 - outside[1:-1, 1:-1])


def find_touching_fibers(image: np.ndarray, max_fibers: int = 10) -> list[tuple[float, float, float]]:
    """Fibers that touch each other or run off the image, as (cx, cy, radius),
    the best seen first.

    1. The biggest one: the deepest point inside the bright regions (dark
       blobs inside the fibers filled in), refined by fitting a circle to the
       cladding edge (fit_cladding_edge).
    2. The others: circles of the same size, found by edges pointing at
       their centres (a Hough transform with the radius fixed), so a fiber
       counts even if only part of its edge shows.
    """
    gray = cv2.cvtColor(as_bgr(image), cv2.COLOR_BGR2GRAY)
    blur = cv2.GaussianBlur(gray, (0, 0), max(1.0, min(gray.shape) / 300))
    _, mask = cv2.threshold(blur, 0, 255, cv2.THRESH_BINARY + cv2.THRESH_OTSU)
    mask = cv2.morphologyEx(_fill_holes(mask), cv2.MORPH_OPEN, cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (9, 9)))
    # Distance to the nearest dark pixel, counting outside the image as dark.
    depth = cv2.distanceTransform(cv2.copyMakeBorder(mask, 1, 1, 1, 1, cv2.BORDER_CONSTANT, value=0),
                                  cv2.DIST_L2, 5)[1:-1, 1:-1]
    y, x = np.unravel_index(int(np.argmax(depth)), depth.shape)
    if depth[y, x] < min(gray.shape) * 0.05:
        return []
    first = fit_cladding_edge(gray, float(x), float(y), float(depth[y, x]))
    if first is None:
        return []
    found = [first]
    for cx, cy in _fixed_radius_centres(gray, first[2], max_fibers):
        if all(math.hypot(cx - fx, cy - fy) > 1.5 * first[2] for fx, fy, _ in found):
            found.append((cx, cy, first[2]))
    return found[:max_fibers]


def fit_cladding_edge(gray: np.ndarray, cx: float, cy: float, radius: float,
                      rays: int = 360) -> tuple[float, float, float] | None:
    """The cladding circle near (cx, cy, radius).

    Along each ray from the centre, the outermost point where the brightness
    falls from above halfway between that ray's own cladding level (just
    inside the edge) and the background to below it, so a shaded, soft side
    of the fiber counts, and dark dirt well inside the edge doesn't. The
    point is then moved to the steepest drop within 6% of the radius. A
    circle is fitted to those points, leaving out stray ones (a touching
    neighbour, dirt on the edge, a glow outside it); repeated 4 times.
    """
    smooth = cv2.GaussianBlur(gray, (0, 0), max(1.5, radius / 150)).astype(np.float32)
    h, w = gray.shape
    background = float(np.percentile(smooth, 5))
    a = np.linspace(0, 2 * np.pi, rays, endpoint=False)
    every = np.arange(rays)
    for _ in range(4):
        t = np.arange(0.5 * radius, 1.35 * radius)
        xs = (cx + np.outer(np.cos(a), t)).astype(np.float32)
        ys = (cy + np.outer(np.sin(a), t)).astype(np.float32)
        inside = (xs >= 0) & (xs < w - 1) & (ys >= 0) & (ys < h - 1)
        profile = cv2.remap(smooth, xs, ys, cv2.INTER_LINEAR)
        level = np.median(profile[:, (t >= 0.6 * radius) & (t <= 0.85 * radius)], axis=1)
        above = (profile > ((level + background) / 2)[:, None]) & inside
        falls = above[:, :-1] & ~above[:, 1:] & inside[:, 1:]
        # Rays with a fall on them, through cladding clearly brighter than the background.
        usable = falls.any(axis=1) & (level - background > 0.25 * (np.median(level) - background))
        last = falls.shape[1] - 1 - np.argmax(falls[:, ::-1], axis=1)
        slope = np.gradient(profile, axis=1)  # outward
        window = int(max(3, 0.06 * radius))
        k = last.copy()
        for i in np.nonzero(usable)[0]:
            lo, hi = max(0, last[i] - window), min(len(t), last[i] + window + 1)
            k[i] = lo + int(np.argmin(slope[i, lo:hi]))
        px, py = xs[every, k][usable].astype(np.float64), ys[every, k][usable].astype(np.float64)
        if len(px) < 20:
            return None
        fit = _fit_circle(px, py)
        if fit is None:
            return None
        cx, cy, radius = fit
    return cx, cy, radius


def _fit_circle(px: np.ndarray, py: np.ndarray, min_points: int = 20) -> tuple[float, float, float] | None:
    """Least-squares circle through the points, leaving out stray ones."""
    keep = np.ones(len(px), bool)
    for _ in range(5):
        if keep.sum() < min_points:
            return None
        # x^2 + y^2 = 2 a x + 2 b y + c
        m = np.column_stack([2 * px[keep], 2 * py[keep], np.ones(keep.sum())])
        (a, b, c), *_ = np.linalg.lstsq(m, px[keep] ** 2 + py[keep] ** 2, rcond=None)
        r = math.sqrt(max(c + a * a + b * b, 1.0))
        off = np.hypot(px - a, py - b) - r
        keep = np.abs(off) < 2.5 * (1.4826 * np.median(np.abs(off[keep])) + 0.5)
    return float(a), float(b), float(r)


def _fixed_radius_centres(gray: np.ndarray, radius: float, max_fibers: int) -> list[tuple[float, float]]:
    """Centres of circles of this radius (also off the image), strongest first:
    each strong edge votes for the point one radius away on its bright side."""
    scale = min(1.0, 60 / radius)  # a radius of 60 px is plenty to place the centre
    small = cv2.resize(gray, None, fx=scale, fy=scale, interpolation=cv2.INTER_AREA).astype(np.float32)
    small = cv2.GaussianBlur(small, (0, 0), 1.5)
    gx, gy = cv2.Sobel(small, cv2.CV_32F, 1, 0), cv2.Sobel(small, cv2.CV_32F, 0, 1)
    strength = np.hypot(gx, gy)
    ys, xs = np.nonzero(strength > np.percentile(strength, 90))
    ux, uy, weight = gx[ys, xs] / strength[ys, xs], gy[ys, xs] / strength[ys, xs], strength[ys, xs]
    r = radius * scale
    pad = int(r * 1.1)
    votes = np.zeros((small.shape[0] + 2 * pad, small.shape[1] + 2 * pad), np.float32)
    for rr in np.linspace(0.97 * r, 1.03 * r, 5):
        vx = np.round(xs + ux * rr).astype(int) + pad
        vy = np.round(ys + uy * rr).astype(int) + pad
        ok = (vx >= 0) & (vx < votes.shape[1]) & (vy >= 0) & (vy < votes.shape[0])
        np.add.at(votes, (vy[ok], vx[ok]), weight[ok])
    votes = cv2.GaussianBlur(votes, (0, 0), max(1.0, r * 0.03))
    top = float(votes.max())
    found = []
    while len(found) < max_fibers and top > 0:
        y, x = np.unravel_index(int(np.argmax(votes)), votes.shape)
        if votes[y, x] < 0.12 * top:  # weaker than this: stray edges, not a fiber
            break
        found.append(((x - pad) / scale, (y - pad) / scale))
        cv2.circle(votes, (int(x), int(y)), int(r * 1.6), 0, -1)
    return found


def _find_round_regions(image: np.ndarray) -> list[tuple[float, float, float]]:
    gray = cv2.cvtColor(as_bgr(image), cv2.COLOR_BGR2GRAY)
    gray = cv2.GaussianBlur(gray, (0, 0), max(1.0, min(gray.shape) / 300))
    _, bright = cv2.threshold(gray, 0, 255, cv2.THRESH_BINARY + cv2.THRESH_OTSU)
    min_r = min(gray.shape) * 0.05
    found = []
    for mask in (bright, 255 - bright):  # fibers lighter, or darker, than the background
        contours, _ = cv2.findContours(mask, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_NONE)
        for c in contours:
            area = cv2.contourArea(c)  # outer outline: holes (rods, core) don't matter
            perimeter = cv2.arcLength(c, True)
            if area < math.pi * min_r ** 2 or perimeter == 0:
                continue
            (x, y), r = cv2.minEnclosingCircle(c)
            roundness = 4 * math.pi * area / perimeter ** 2
            fill = area / (math.pi * r * r)  # 1 for a perfect disc
            touches_edge = x - r < 1 or y - r < 1 or x + r > gray.shape[1] - 1 or y + r > gray.shape[0] - 1
            if roundness > 0.75 and fill > 0.8 and not touches_edge:
                found.append((float(x), float(y), float(math.sqrt(area / math.pi))))
        if found:
            break
    return sorted(found, key=lambda c: -c[2])


def _find_hough_circles(image: np.ndarray, max_fibers: int = 10) -> list[tuple[float, float, float]]:
    gray = cv2.cvtColor(as_bgr(image), cv2.COLOR_BGR2GRAY)
    h, w = gray.shape
    scale = min(1.0, 800 / max(h, w))  # detect on a smaller copy: faster, less noise
    small = cv2.resize(gray, (round(w * scale), round(h * scale)), interpolation=cv2.INTER_AREA)
    small = cv2.medianBlur(small, 5)
    short = min(small.shape)
    circles = cv2.HoughCircles(small, cv2.HOUGH_GRADIENT, dp=1.5, minDist=short * 0.15,
                               param1=120, param2=40, minRadius=round(short * 0.06),
                               maxRadius=round(short * 0.5))
    if circles is None:
        return []
    found = []
    for x, y, r in sorted(circles[0], key=lambda c: -c[2]):
        x, y, r = x / scale, y / scale, r / scale
        # Skip circles inside one already kept (e.g. a stress rod).
        if any(math.hypot(x - fx, y - fy) < fr for fx, fy, fr in found):
            continue
        found.append((float(x), float(y), float(r)))
        if len(found) == max_fibers:
            break
    return found


def contrast_map(crop: np.ndarray, cx: float, cy: float, radius: float,
                 detection: Detection) -> tuple[np.ndarray, np.ndarray]:
    """How different each pixel is from the cladding, in noise units, and the
    ring (between core and cladding edge) it's measured in. (cx, cy) is the
    fiber centre in crop's pixels."""
    h, w = crop.shape[:2]
    yy, xx = np.mgrid[:h, :w]
    u, v = (xx - cx) / radius, (yy - cy) / radius
    dist = np.hypot(u, v)
    ring = (dist > CORE_FRACTION) & (dist < EDGE_FRACTION)
    if ring.sum() < 50:
        return np.zeros((h, w), np.float32), ring
    lab = cv2.cvtColor(crop, cv2.COLOR_BGR2LAB).astype(np.float32)
    blur = radius * detection.smoothing / 100
    # Tilt plus a round bowl: covers uneven lighting and vignetting, but nothing
    # with an axis of its own, so it can't fit away a pair of stress parts.
    surface = np.stack([np.ones_like(u), u, v, dist * dist], axis=-1).astype(np.float32)
    step = max(1, int(math.sqrt(ring.sum() / FIT_SAMPLES)))  # fit on a sparse grid: much faster
    sample_ring, sample_surface = ring[::step, ::step], surface[::step, ::step]
    score = np.zeros((h, w), np.float32)
    for i in LOOK_AT.get(detection.look_at, LOOK_AT["both"])[1]:
        channel = cv2.GaussianBlur(lab[..., i], (0, 0), blur) if blur >= 0.3 else lab[..., i]
        sample = channel[::step, ::step]
        # Fit to the cladding: start from the commonest value (the cladding covers
        # most of the ring), then refit leaving out pixels far off the surface.
        values = sample[sample_ring]
        hist, edges = np.histogram(values, bins=max(1, int(np.ptp(values))), range=(values.min(), values.max() + 1))
        common = edges[int(np.argmax(np.convolve(hist, np.ones(3), "same")))]
        step_noise = 1.4826 * float(np.median(np.abs(np.diff(sample, axis=1)[sample_ring[:, 1:]]))) / math.sqrt(2)
        keep = sample_ring & (np.abs(sample - common) < max(3 * step_noise, 4 * NOISE_FLOOR))
        for _ in range(5):
            coef, *_ = np.linalg.lstsq(sample_surface[keep], sample[keep], rcond=None)
            off = sample - sample_surface @ coef
            noise = 1.4826 * float(np.median(np.abs(off[keep]))) + NOISE_FLOOR  # robust standard deviation
            keep = sample_ring & (np.abs(off) < 2 * noise)
        score += ((channel - surface @ coef) / noise) ** 2
    return np.sqrt(score), ring


def _sample(g: np.ndarray, xs: np.ndarray, ys: np.ndarray, width: int = 4096) -> np.ndarray:
    """g at points (xs, ys) of any shape, between pixels; in blocks, as cv2.remap
    takes maps narrower than 32767 px."""
    fx, fy = xs.reshape(-1).astype(np.float32), ys.reshape(-1).astype(np.float32)
    n = len(fx)
    rows = max(1, -(-n // width))
    pad = rows * width - n
    out = cv2.remap(g, np.pad(fx, (0, pad)).reshape(rows, width), np.pad(fy, (0, pad)).reshape(rows, width),
                    cv2.INTER_LINEAR)
    return out.reshape(-1)[:n].reshape(xs.shape)


def _rod_gradients(image: np.ndarray, cx: float, cy: float, radius: float):
    """The fiber scaled to a radius of SYMMETRY_SIZE px, blurred, as x and y
    gradients, with the cladding edge and anything off the image zeroed.
    Returns gx, gy, half (the fiber centre's pixel), scale, typical (the
    median gradient size inside: mostly noise)."""
    gray = cv2.cvtColor(as_bgr(image), cv2.COLOR_BGR2GRAY).astype(np.float32)
    scale = SYMMETRY_SIZE / radius
    half = int(SYMMETRY_SIZE * 1.05)
    to_crop = np.float32([[scale, 0, half - cx * scale], [0, scale, half - cy * scale]])
    size = (2 * half + 1, 2 * half + 1)
    crop = cv2.warpAffine(gray, to_crop, size, flags=cv2.INTER_AREA if scale < 1 else cv2.INTER_LINEAR)
    crop = cv2.GaussianBlur(crop, (0, 0), 1.5)
    gx, gy = cv2.Sobel(crop, cv2.CV_32F, 1, 0) / 8, cv2.Sobel(crop, cv2.CV_32F, 0, 1) / 8
    # Parts of the fiber off the image: no edges there (not even the image border).
    in_image = cv2.warpAffine(np.full(gray.shape, 255, np.uint8), to_crop, size, flags=cv2.INTER_NEAREST)
    in_image = cv2.erode(in_image, np.ones((7, 7), np.uint8))
    yy, xx = np.mgrid[:size[1], :size[0]]
    off = (np.hypot(xx - half, yy - half) > ROD_SEARCH_EDGE * SYMMETRY_SIZE) | (in_image == 0)
    gx[off] = 0
    gy[off] = 0
    typical = max(float(np.median(np.hypot(gx, gy)[~off])), GRADIENT_FLOOR)  # floor: clean, drawn images
    return gx, gy, half, scale, typical


def _ring_score(gx: np.ndarray, gy: np.ndarray, cap: float, qx, qy, rho, samples: int,
                exact: bool = False) -> np.ndarray:
    """How well each circle (centres qx, qy, radii rho: arrays of one shape)
    follows edges: the mean, round the circle, of the gradient across it,
    each sample counting at most cap. With the cap, a circle scores by how
    much of it lies on an edge, so a few strong dust edges can't win."""
    a = np.linspace(0, 2 * np.pi, samples, endpoint=False)
    ca, sa = np.cos(a), np.sin(a)
    xs = np.asarray(qx)[..., None] + np.asarray(rho)[..., None] * ca
    ys = np.asarray(qy)[..., None] + np.asarray(rho)[..., None] * sa
    if exact:  # in between pixels
        sx, sy = _sample(gx, xs, ys), _sample(gy, xs, ys)
    else:  # nearest pixel: fast, for the wide search
        h, w = gx.shape
        xi, yi = np.clip(np.round(xs).astype(int), 0, w - 1), np.clip(np.round(ys).astype(int), 0, h - 1)
        sx, sy = gx[yi, xi], gy[yi, xi]
    return np.minimum(np.abs(sx * ca + sy * sa), cap).mean(-1)


def _pair_score(gx, gy, cap, mx, my, angle, spacing, rho, samples, exact=False, both=np.minimum):
    """_ring_score of two circles of radius rho, spacing either side of (mx, my) at angle (radians)."""
    dx, dy = spacing * np.cos(angle), -spacing * np.sin(angle)  # screen y points down
    return both(_ring_score(gx, gy, cap, mx + dx, my + dy, rho, samples, exact),
                _ring_score(gx, gy, cap, mx - dx, my - dy, rho, samples, exact))


def find_rods(image: np.ndarray, cx: float, cy: float, radius: float):
    """Panda stress rods of the fiber (cx, cy, radius): two round outlines.

    1. The fiber is scaled to a radius of SYMMETRY_SIZE px. Every pair of
       equal circles either side of the centre is tried (angle in 2 degree
       steps, spacing ROD_SPACINGS and size ROD_SIZES of the radius) and
       scored by _ring_score; the pair's score is its weaker circle's.
    2. The best pair is refined as one shape: the point between the rods
       (the core, which needn't be the cladding centre), angle, spacing and
       size. Rods are symmetric about the core, so dirt over one rod can't
       pull it away on its own.
    3. Each rod's centre is fitted to a fraction of a pixel: a circle through
       the strongest edge near it on each ray, stray points left out,
       repeated 3 times. A rod whose own outline hardly shows (dirt, shade)
       is placed opposite the other one instead.
    4. Otherwise the line between the rods is refined by matching one rod's
       outline against the other (_match_rods), if that matches well and
       agrees with step 3 to within MATCH_AGREE degrees. Both rods are lit
       the same way, so even lopsided (embossed) outlines match exactly.

    Returns ([(x, y, r), (x, y, r)] in image pixels, quality, coverage):
    quality is the pair's score over a typical pair's (MIN_ROD_QUALITY);
    coverage is how much of the weaker rod's outline has an edge right on it
    (MIN_ROD_COVERAGE, see _edge_coverage).
    """
    return _find_rods(_rod_gradients(image, cx, cy, radius), cx, cy)[:3]


def _find_rods(grads, cx: float, cy: float):
    """find_rods on _rod_gradients' result, plus the turned gain (_match_rods)."""
    gx, gy, half, scale, typical = grads
    cap = EDGE_CAP * typical
    size = SYMMETRY_SIZE
    angles, spacings, radii = np.meshgrid(np.radians(np.arange(0, 180, 2.0)),
                                          np.arange(ROD_SPACINGS[0], ROD_SPACINGS[1], 0.02) * size,
                                          np.arange(ROD_SIZES[0], ROD_SIZES[1], 0.015) * size, indexing="ij")
    ok = (spacings + radii < ROD_MAX_REACH * size) & (spacings - radii > 0.05 * size)  # inside, apart
    angles, spacings, radii = angles[ok], spacings[ok], radii[ok]
    score = _pair_score(gx, gy, cap, half, half, angles, spacings, radii, 64)
    best = int(np.argmax(score))
    quality = float(score[best] / max(np.median(score), 1e-6))

    # 2. The pair as one shape: midpoint x, y, angle, spacing, size. Refined
    # from the ROD_STARTS best different pairs of step 1, keeping the best
    # result, so a slightly different start can't end somewhere else.
    starts = []
    for i in np.argsort(-score):
        if all(abs((math.degrees(angles[i] - angles[j]) + 90) % 180 - 90) > 4 or abs(spacings[i] - spacings[j]) > 0.05 * size
               or abs(radii[i] - radii[j]) > 0.04 * size for j in starts):
            starts.append(i)
            if len(starts) == ROD_STARTS:
                break
    results = []
    for i in starts:
        p = np.array([half, half, angles[i], spacings[i], radii[i]], float)
        steps = np.array([2.0, 2.0, math.radians(1.0), 2.0, 2.0])
        for _ in range(6):
            for _ in range(3):
                for k in range(5):
                    tries = np.repeat(p[None], 5, axis=0)
                    tries[:, k] += np.arange(-2, 3) * steps[k]
                    p = tries[int(np.argmax(_pair_score(gx, gy, cap, *tries.T, 128, exact=True, both=np.add)))]
            steps /= 2
        results.append((float(_pair_score(gx, gy, cap, *p, 256, exact=True)), p))
    mx, my, angle, spacing, r = max(results, key=lambda res: res[0])[1]
    dx, dy = spacing * math.cos(angle), -spacing * math.sin(angle)
    centres = [(mx + dx, my + dy), (mx - dx, my - dy)]

    # 3. Each centre to a fraction of a pixel.
    fitted, sizes = [], []
    for x0, y0 in centres:
        x, y, rr = x0, y0, r
        for _ in range(3):
            fit = _fit_rod_edge(gx, gy, x, y, rr, 0.12)
            if fit is None or math.hypot(fit[0] - x0, fit[1] - y0) > 0.1 * r or abs(fit[2] - r) > 0.15 * r:
                break
            x, y, rr = fit
        fitted.append((x, y))
        sizes.append(rr)
    r = float(np.mean(sizes))  # one size for both
    shows = [_edge_coverage(gx, gy, x, y, r, typical) for x, y in fitted]
    weak, strong = int(np.argmin(shows)), int(np.argmax(shows))
    shift, turned_gain = _match_rods(gx, gy, typical, fitted, r)
    if shows[weak] < 0.5 <= shows[strong]:  # one rod hardly shows: opposite the other
        sx, sy = fitted[strong]
        fitted[weak] = (2 * mx - sx, 2 * my - sy)
    else:
        # 4. The rods look alike (same size, same light): where rod 0's outline
        # matches rod 1 best gives the rod-to-rod line more exactly than the
        # edge fits, if the match is good and agrees with them.
        if shift is not None:
            (x0, y0), (x1, y1) = fitted
            vx, vy = shift
            if abs((math.degrees(math.atan2(vy, vx) - math.atan2(y1 - y0, x1 - x0)) + 90) % 180 - 90) <= MATCH_AGREE:
                px, py = (x0 + x1) / 2, (y0 + y1) / 2
                fitted = [(px - vx / 2, py - vy / 2), (px + vx / 2, py + vy / 2)]
    rods = [((x - half) / scale + cx, (y - half) / scale + cy, r / scale) for x, y in fitted]
    return rods, quality, min(shows), turned_gain


def _match_rods(gx, gy, typical: float, centres, r: float, search: int = 16):
    """Rod 0 to rod 1 as (dx, dy) at the search scale, found by sliding the
    edge strength in a ring round rod 0's outline (0.75-1.25 r, so dirt
    inside the rod is left out) over rod 1, to a fraction of a pixel; None if
    it matches poorly (MATCH_SCORE). Also how much better rod 0 matches rod 1
    turned 180 degrees than shifted ("turned gain"): about 0 or less for
    Panda rods, which look alike side by side, clearly more for Bow-tie
    wedges or the two ends of an ellipse, which point opposite ways.
    Returns (shift or None, turned gain), or (None, None) off the map."""
    strength = np.minimum(np.hypot(gx, gy), EDGE_CAP * typical).astype(np.float32)
    (ax, ay), (bx, by) = centres
    size = int(math.ceil(1.25 * r)) + 1
    yy, xx = np.mgrid[-size:size + 1, -size:size + 1]
    ring = np.hypot(xx, yy)
    ring = ((ring >= 0.75 * r) & (ring <= 1.25 * r)).astype(np.float32)
    ai, aj, bi, bj = int(round(ay)), int(round(ax)), int(round(by)), int(round(bx))
    h, w = strength.shape
    if (min(ai, aj, bi, bj) - size - search < 0 or max(ai, bi) + size + search >= h
            or max(aj, bj) + size + search >= w):
        return None, None
    template = strength[ai - size:ai + size + 1, aj - size:aj + size + 1]
    area = strength[bi - size - search:bi + size + search + 1, bj - size - search:bj + size + search + 1]
    match = np.nan_to_num(cv2.matchTemplate(area, template, cv2.TM_CCORR_NORMED, mask=ring))
    turned = np.nan_to_num(cv2.matchTemplate(area, np.ascontiguousarray(template[::-1, ::-1]),
                                             cv2.TM_CCORR_NORMED, mask=ring))
    i, j = np.unravel_index(int(np.argmax(match)), match.shape)
    gain = float(turned.max() - match[i, j])
    if match[i, j] < MATCH_SCORE:
        return None, gain

    def peak(v, k):  # parabola through the peak and its neighbours
        if 0 < k < len(v) - 1 and v[k - 1] - 2 * v[k] + v[k + 1] < 0:
            return 0.5 * (v[k - 1] - v[k + 1]) / (v[k - 1] - 2 * v[k] + v[k + 1])
        return 0.0
    return (bj - aj + j + peak(match[i, :], j) - search, bi - ai + i + peak(match[:, j], i) - search), gain


def _fit_rod_edge(gx, gy, x, y, r, spread: float = 0.08, rays: int = 180):
    """Circle fitted to the strongest edge within spread x r of the circle along each ray from (x, y)."""
    a = np.linspace(0, 2 * np.pi, rays, endpoint=False)
    ca, sa = np.cos(a), np.sin(a)
    radii = r * np.linspace(1 - spread, 1 + spread, 33)
    xs, ys = (x + np.outer(ca, radii)).astype(np.float32), (y + np.outer(sa, radii)).astype(np.float32)
    across = np.abs(cv2.remap(gx, xs, ys, cv2.INTER_LINEAR) * ca[:, None] + cv2.remap(gy, xs, ys, cv2.INTER_LINEAR) * sa[:, None])
    k = np.argmax(across, axis=1)
    strong = across[np.arange(rays), k] > np.median(across)  # rays with a real edge on them
    if strong.sum() < rays // 4:
        return None
    return _fit_circle(xs[np.arange(rays), k][strong].astype(np.float64), ys[np.arange(rays), k][strong].astype(np.float64))


def _edge_coverage(gx, gy, x, y, r, typical: float, n_arcs: int = 32) -> float:
    """Fraction of the circle's outline (in n_arcs arcs) where a clear edge
    lies right on the circle: on each arc, the strongest edge within 5% of
    the radius must be at least ON_CIRCLE_SHARE of the strongest within 25%.
    Near 1 for a round rod; low for a circle that only crosses edges (e.g.
    the sides of a bow-tie wedge)."""
    per_arc = 8
    a = np.linspace(0, 2 * np.pi, n_arcs * per_arc, endpoint=False)
    ca, sa = np.cos(a), np.sin(a)
    radii = r * np.linspace(0.75, 1.25, 41)
    xs, ys = (x + np.outer(radii, ca)).astype(np.float32), (y + np.outer(radii, sa)).astype(np.float32)
    across = np.abs(cv2.remap(gx, xs, ys, cv2.INTER_LINEAR) * ca + cv2.remap(gy, xs, ys, cv2.INTER_LINEAR) * sa)
    across = across.reshape(len(radii), n_arcs, per_arc).max(axis=2)  # strongest edge per radius, per arc
    near = across[np.abs(radii - r) <= 0.05 * r].max(axis=0)
    return float(((near >= ON_CIRCLE_SHARE * across.max(axis=0)) & (near > 2 * typical)).mean())


def _boundary_fit(gx, gy, cap, xs, ys, nx, ny) -> np.ndarray:
    """How well outlines follow edges: mean over the last axis of the capped
    gradient across them (points xs, ys with unit normals nx, ny)."""
    return np.minimum(np.abs(_sample(gx, xs, ys) * nx + _sample(gy, xs, ys) * ny), cap).mean(-1)


def shape_fits(grads, cx: float, cy: float, rods) -> dict[str, float]:
    """How well each kind of stress part fits the edges, for the "auto" method:
    the rods found (Panda), the best pair of wedges (Bow-tie: annular sectors
    either side of the core) and the best ellipse round the core (Elliptical),
    both searched along the rods' axis (+-6 degrees). Each is the mean capped
    gradient across its outline, over the cap (0-1), so they compare fairly."""
    gx, gy, half, scale, typical = grads
    cap = EDGE_CAP * typical
    size = SYMMETRY_SIZE
    (ax, ay, ar), (bx, by, br) = rods
    theta = math.atan2(-(by - ay), bx - ax)
    hx, hy = ((ax + bx) / 2 - cx) * scale + half, ((ay + by) / 2 - cy) * scale + half
    t = np.linspace(0, 2 * np.pi, 96, endpoint=False)
    xs = np.concatenate([(x - cx) * scale + half + r * scale * np.cos(t) for x, y, r in rods])
    ys = np.concatenate([(y - cy) * scale + half + r * scale * np.sin(t) for x, y, r in rods])
    panda = float(_boundary_fit(gx, gy, cap, xs, ys, np.tile(np.cos(t), 2), np.tile(np.sin(t), 2)))

    # Wedges: inner radius, outer radius, half-angle; two arcs and two straight sides each.
    turn, r1, r2, half_angle = (v[..., None] for v in np.meshgrid(
        np.radians([-6, -3, 0, 3, 6]), np.arange(0.03, 0.42, 0.03) * size, np.arange(0.35, 0.9, 0.03) * size,
        np.radians(np.arange(8, 62, 3)), indexing="ij"))
    keep = (r2 > r1 + 0.15 * size)[..., 0]
    turn, r1, r2, half_angle = turn[keep], r1[keep], r2[keep], half_angle[keep]
    s, u = np.linspace(-1, 1, 16), np.linspace(0, 1, 16)
    parts = []
    for base in (0.0, math.pi):
        b = theta + turn + base
        for r in (r1, r2):  # arcs
            a = b + s * half_angle
            parts.append((hx + r * np.cos(a), hy - r * np.sin(a), np.cos(a), -np.sin(a)))
        for side in (-1, 1):  # straight sides
            a = b + side * half_angle
            rr = r1 + (r2 - r1) * u
            parts.append((hx + rr * np.cos(a), hy - rr * np.sin(a), np.broadcast_to(-np.sin(a), rr.shape),
                          np.broadcast_to(-np.cos(a), rr.shape)))
    bowtie = float(_boundary_fit(gx, gy, cap, *(np.concatenate([q[i] for q in parts], 1) for i in range(4))).max())

    # Ellipses: long axis along the rods' axis or across it.
    turn, across, a_len, b_len = (v[..., None] for v in np.meshgrid(
        np.radians([-6, -3, 0, 3, 6]), [0.0, math.pi / 2], np.arange(0.25, 0.85, 0.03) * size,
        np.arange(0.12, 0.6, 0.03) * size, indexing="ij"))
    keep = (b_len < 0.95 * a_len)[..., 0]
    turn, across, a_len, b_len = turn[keep], across[keep], a_len[keep], b_len[keep]
    e = np.linspace(0, 2 * np.pi, 64, endpoint=False)
    ct, st = np.cos(theta + turn + across), np.sin(theta + turn + across)
    pu, pv = a_len * np.cos(e), b_len * np.sin(e)
    nu, nv = np.cos(e) / a_len, np.sin(e) / b_len
    nx, ny = nu * ct - nv * st, -(nu * st + nv * ct)
    norm = np.hypot(nx, ny)
    ellipse = float(_boundary_fit(gx, gy, cap, hx + pu * ct - pv * st, hy - (pu * st + pv * ct),
                                  nx / norm, ny / norm).max())
    return {"Panda": panda / cap, "Bow-tie": bowtie / cap, "Elliptical": ellipse / cap}


def _measure_auto(image: np.ndarray, cx: float, cy: float, radius: float, detection: Detection | None) -> FiberAxis:
    """The "auto" method: Panda rod outlines, unless the edges are strong
    (OTHER_SHAPE_QUALITY), the two parts point opposite ways (TURNED_GAIN)
    and wedges or an ellipse fit the edges better (OTHER_SHAPE_MARGIN); then
    mirror symmetry. Leans to Panda: a Bow-tie
    measured by its rods is off by about a degree, a dirty Panda measured by
    mirror symmetry by much more."""
    grads = _rod_gradients(image, cx, cy, radius)
    found = _measure_rods(image, cx, cy, radius, grads)
    if found.quality >= SURE_PANDA[0] and found.coverage >= SURE_PANDA[1]:
        return found  # plainly round rods
    if found.quality < OTHER_SHAPE_QUALITY:
        return found  # nothing strong found: dirt, not wedges (unclear if below MIN_ROD_QUALITY)
    if found.turned_gain is None or found.turned_gain < TURNED_GAIN:
        return found  # the two parts look alike side by side: rods
    rods = [(x, y, r) for (x, y), r in zip(found.part_centres, found.rod_radii)]
    fits = shape_fits(grads, cx, cy, rods)
    other = max(("Bow-tie", "Elliptical"), key=fits.get)
    if fits[other] < fits["Panda"] + OTHER_SHAPE_MARGIN:
        return found
    result = measure_axis(image, cx, cy, radius, "symmetry", detection=detection)
    result.fiber_type = other
    return result


def _measure_rods(image: np.ndarray, cx: float, cy: float, radius: float, grads=None) -> FiberAxis:
    """The "rods" method."""
    rods, quality, coverage, turned_gain = _find_rods(grads or _rod_gradients(image, cx, cy, radius), cx, cy)
    (ax, ay, ar), (bx, by, br) = rods
    angle = math.degrees(math.atan2(-(by - ay), bx - ax)) % 180  # y flipped
    # Spread of two discs along / across the line through them.
    half_gap, rod = math.hypot(bx - ax, by - ay) / 2, (ar + br) / 2
    elongation = math.sqrt(1 + 4 * half_gap ** 2 / rod ** 2)
    a = np.linspace(0, 2 * np.pi, 72, endpoint=False)
    outlines = [np.column_stack([x + r * np.cos(a), y + r * np.sin(a)]) for x, y, r in rods]
    found = FiberAxis(cx, cy, radius, angle, elongation, "?", 2, "rods", (ax + bx) / 2, (ay + by) / 2,
                      outlines, [(ax, ay), (bx, by)], quality=quality, coverage=coverage, rod_radii=[ar, br],
                      turned_gain=turned_gain)
    if found.clear:
        found.fiber_type = "Panda"
    return found


def _otsu(values: np.ndarray) -> float:
    """Otsu's threshold of float values (the top 0.1% clipped off)."""
    top = float(np.percentile(values, 99.9))
    if top <= 0:
        return 0.0
    hist, edges = np.histogram(np.clip(values, 0, top), bins=256, range=(0, top))
    p = hist / hist.sum()
    mids = (edges[:-1] + edges[1:]) / 2
    w0 = np.cumsum(p)
    m0 = np.cumsum(p * mids)
    between = (m0[-1] * w0 - m0) ** 2 / np.maximum(w0 * (1 - w0), 1e-12)
    return float(edges[int(np.argmax(between[:-1])) + 1])


def measure_axis(image: np.ndarray, cx: float, cy: float, radius: float,
                 method: str = "auto", parts_centre: bool = False,
                 detection: Detection | None = None) -> FiberAxis:
    """The slow axis of the fiber with cladding circle (cx, cy, radius)."""
    if method == "auto":
        return _measure_auto(image, cx, cy, radius, detection)
    if method == "rods":
        return _measure_rods(image, cx, cy, radius)
    detection = detection or Detection()
    img = as_bgr(image)
    h, w = img.shape[:2]
    x0, x1 = max(0, int(cx - radius)), min(w, int(cx + radius) + 1)
    y0, y1 = max(0, int(cy - radius)), min(h, int(cy + radius) + 1)
    crop = img[y0:y1, x0:x1]
    yy, xx = np.mgrid[y0:y1, x0:x1]
    dx, dy = xx - cx, yy - cy
    diff, ring = contrast_map(crop, cx - x0, cy - y0, radius, detection)
    if ring.sum() < 50:
        return FiberAxis(cx, cy, radius, 0.0, 1.0, "?", 0)
    if detection.threshold is None:
        threshold = max(_otsu(diff[ring]), MIN_AUTO_THRESHOLD)
    else:
        threshold = detection.threshold
    mask = ring & (diff > threshold)

    # Principal axis of the stress parts about the fiber centre.
    wx, wy = dx[mask], -dy[mask]  # y flipped: angles as seen on screen
    px, py = cx, cy  # the point the axis turns about
    if parts_centre and len(wx):
        px, py = cx + wx.mean(), cy - wy.mean()
        wx, wy = wx - wx.mean(), wy - wy.mean()
    if len(wx) < 20:
        return FiberAxis(cx, cy, radius, 0.0, 1.0, "?", 0, threshold=threshold)
    mu20, mu02, mu11 = (wx * wx).mean(), (wy * wy).mean(), (wx * wy).mean()
    angle = 0.5 * math.degrees(math.atan2(2 * mu11, mu20 - mu02)) % 180
    spread = math.sqrt(((mu20 - mu02) / 2) ** 2 + mu11 ** 2)
    along, across = (mu20 + mu02) / 2 + spread, (mu20 + mu02) / 2 - spread
    elongation = math.sqrt(along / max(across, 1e-9))

    parts = _stress_parts(mask.astype(np.uint8), radius * detection.min_size / 100)
    n_parts, fiber_type = _classify(parts)
    used = "moments"
    if method == "centres" and len(parts) == 2:
        (ax, ay), (bx, by) = (_centroid(c) for c in parts)
        angle = math.degrees(math.atan2(-(by - ay), bx - ax)) % 180  # y flipped
        used = "centres"
    elif method == "symmetry":
        angle = _symmetry_angle(diff, px - x0, py - y0, radius, angle)
        used = "symmetry"
    if used == "centres":
        (ax, ay), (bx, by) = (_centroid(c) for c in parts)
        px, py = x0 + (ax + bx) / 2, y0 + (ay + by) / 2
    elif not parts_centre:
        px = py = None
    # For drawing: outlines simplified to ~0.5% of the radius, in image pixels.
    outlines = [cv2.approxPolyDP(c, max(0.5, radius * 0.005), True).reshape(-1, 2).astype(np.float64) + (x0, y0)
                for c in parts]
    centres = [(x0 + x, y0 + y) for x, y in (_centroid(c) for c in parts)]
    return FiberAxis(cx, cy, radius, angle, elongation, fiber_type, n_parts, used, px, py, outlines, centres,
                     threshold)


def _stress_parts(mask: np.ndarray, min_side: float) -> list[np.ndarray]:
    """Outlines of the stress parts, largest first, specks (smaller than a
    min_side square) left out."""
    mask = cv2.morphologyEx(mask, cv2.MORPH_OPEN, np.ones((3, 3), np.uint8))
    contours, _ = cv2.findContours(mask, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
    min_area = min_side ** 2
    parts = [c for c in contours if cv2.contourArea(c) >= min_area]
    return sorted(parts, key=lambda c: -cv2.contourArea(c))


def _centroid(contour: np.ndarray) -> tuple[float, float]:
    m = cv2.moments(contour)
    return m["m10"] / m["m00"], m["m01"] / m["m00"]


def _classify(parts: list[np.ndarray]) -> tuple[int, str]:
    if len(parts) == 1:
        return 1, "Elliptical"
    if len(parts) == 2:
        # Panda rods are round; bow-tie wedges aren't.
        roundness = [4 * math.pi * cv2.contourArea(c) / max(cv2.arcLength(c, True) ** 2, 1e-9) for c in parts]
        return 2, "Panda" if min(roundness) > 0.8 else "Bow-tie"
    return len(parts), "?"


def _symmetry_angle(diff: np.ndarray, cx: float, cy: float, radius: float, start_deg: float) -> float:
    """The axis angle (degrees, 0-180) the map is most mirror-symmetric about,
    together with the line at 90 degrees to it, searched near start_deg.
    (cx, cy) is the fiber centre in diff's pixels."""
    scale = min(1.0, SYMMETRY_SIZE / radius)
    half = int(radius * EDGE_FRACTION * scale)
    size = 2 * half + 1  # odd, so the centre is a pixel and flipping keeps it in place
    yy, xx = np.mgrid[-half:half + 1, -half:half + 1]
    d = np.hypot(xx, yy)
    disk = (d < half) & (d > radius * scale * CORE_FRACTION)
    src = diff.astype(np.float32)

    def error(deg: float) -> float:
        # Scale and turn the map so the candidate axis is horizontal and the
        # fiber centre is the middle pixel, then compare with both mirror images.
        m = cv2.getRotationMatrix2D((cx, cy), -deg, scale)  # -deg: on screen y points down
        m[0, 2] += half - cx
        m[1, 2] += half - cy
        rot = cv2.warpAffine(src, m, (size, size), flags=cv2.INTER_LINEAR)
        return float((np.abs(rot - rot[::-1]) + np.abs(rot - rot[:, ::-1]))[disk].mean())

    best = min(np.arange(start_deg - 20, start_deg + 20.01, 1.0), key=error)
    best = min(np.arange(best - 1, best + 1.001, 0.05), key=error)
    return float(best) % 180


def analyse(image: np.ndarray, circles: list[tuple[float, float, float]] | None = None,
            method: str = "auto", parts_centre: bool = False,
            detection: Detection | None = None, centre_only: bool = False) -> list[FiberAxis]:
    """Axes of the given fibers, or of every fiber found in the image.
    centre_only: of the fibers found, just the one nearest the middle of the image
    (given circles are all measured)."""
    if not circles:
        circles = find_fibers(image)
        if centre_only:
            circles = nearest_centre(circles, image.shape)
    return [measure_axis(image, *c, method=method, parts_centre=parts_centre, detection=detection)
            for c in circles]


if __name__ == "__main__":
    from camcontrol.image_io import load_image_file

    path = sys.argv[1] if len(sys.argv) > 1 else str(Path(__file__).resolve().parents[2] / "Fiber Types.png")
    img = load_image_file(path)
    for f in sorted(analyse(img), key=lambda f: f.cx):
        others = "  ".join(f"{m} {measure_axis(img, f.cx, f.cy, f.radius, m).slow_deg:6.2f}" for m in METHODS)
        print(f"fiber at ({f.cx:.0f}, {f.cy:.0f}) r {f.radius:.0f}: {f.fiber_type:10} fast {f.fast_deg:6.1f} deg, "
              f"elongation {f.elongation:.2f}, parts {f.n_parts}, threshold {f.threshold:.1f}; slow axis by method: {others}")

    # Self-test: a drawn Panda fiber rotated to known angles.
    for true_angle in (0, 30, 75, 120):
        test = np.full((400, 400, 3), 30, np.uint8)
        cv2.circle(test, (200, 200), 150, (240, 220, 180), -1)
        a = math.radians(true_angle)
        for s in (-1, 1):
            cv2.circle(test, (round(200 + s * 75 * math.cos(a)), round(200 - s * 75 * math.sin(a))), 35, (200, 120, 30), -1)
        cv2.circle(test, (200, 200), 8, (60, 220, 240), -1)
        for method in METHODS:
            f = measure_axis(test, 200, 200, 150, method)
            err = min(abs(f.slow_deg - true_angle), 180 - abs(f.slow_deg - true_angle))
            used = "rods" if method == "auto" else method  # auto picks the rods for a Panda fiber
            assert err < 2 and f.fiber_type == "Panda" and f.method == used, (true_angle, method, f)
    # Two touching Panda fibers, the right one cut off by the image edge,
    # with a dark blob over one rod (as V-groove images have).
    for true_angle in (0, 20, 100):
        test = np.full((500, 800, 3), 10, np.uint8)
        for fx, lit in ((300, 230), (650, 120)):
            cv2.circle(test, (fx, 250), 175, (lit,) * 3, -1)
            a = math.radians(true_angle)
            for s in (-1, 1):
                cv2.circle(test, (round(fx + s * 85 * math.cos(a)), round(250 - s * 85 * math.sin(a))), 50,
                           (lit + 15,) * 3, 2)
        blob = np.zeros(test.shape[:2], np.float32)
        cv2.circle(blob, (round(300 - 85 * math.cos(a)), round(250 + 85 * math.sin(a))), 35, 80, -1)
        test = np.clip(test - cv2.GaussianBlur(blob, (0, 0), 12)[..., None], 0, 255).astype(np.uint8)
        test = cv2.GaussianBlur(test, (0, 0), 1.5)
        found = analyse(test, method="rods")
        assert len(found) == 2, (true_angle, [(f.cx, f.cy, f.radius) for f in found])
        f = analyse(test, method="rods", centre_only=True)[0]
        err = min(abs(f.slow_deg - true_angle), 180 - abs(f.slow_deg - true_angle))
        assert abs(f.cx - 300) < 3 and abs(f.radius - 175) < 3 and err < 1 and f.clear, (true_angle, f)
    print("fiber_axis OK")
