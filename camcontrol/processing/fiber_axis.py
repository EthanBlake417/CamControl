"""Axis of symmetry of polarization-maintaining (PM) fiber end faces.

PM fibers have stress parts on either side of the core: two round rods
(Panda), two wedges (Bow-tie), or an elliptical region round the core
(Elliptical-clad). The line through them is the fiber's slow axis; the
fast axis is at 90 degrees. Both are mirror-symmetry axes of the end face.

How it's measured, for each fiber:
1. Find the fiber: the round cladding (Hough circle detection), or a
   circle given by the caller (e.g. a Circle measurement).
2. Find the stress parts: inside the cladding, the pixels clearly different
   from the cladding (see contrast_map). For each Lab channel a smooth
   surface (tilt plus round bowl, fitted to the cladding only) is taken off, so uneven
   lighting doesn't count, and the rest is divided by the cladding's noise
   level. So a faint brightness-only difference counts as much as a strong
   color one. The contrast is in noise units; the threshold is automatic
   (Otsu) or set by the user (Detection). The core, in the middle, and the
   edge of the cladding are left out.
3. The slow axis through them, by one of three methods (METHODS):
   - "symmetry": the line the end face is most mirror-symmetric about
     (with the line at 90 degrees, as both are mirror axes). Uses the
     contrast map directly, so no threshold, and lopsided stress parts
     don't pull it. The default.
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

METHODS = {  # name: label in the panel
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
        return self.elongation >= MIN_ELONGATION


def find_fibers(image: np.ndarray, max_fibers: int = 10) -> list[tuple[float, float, float]]:
    """Round fibers in the image as (cx, cy, radius), largest first.

    First tries round regions that are clearly brighter (or darker) than the
    background; if there are none, falls back to circle-edge detection.
    """
    found = _find_round_regions(image)
    if not found:
        found = _find_hough_circles(image)
    return found[:max_fibers]


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
                 method: str = "symmetry", parts_centre: bool = False,
                 detection: Detection | None = None) -> FiberAxis:
    """The slow axis of the fiber with cladding circle (cx, cy, radius)."""
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
            method: str = "symmetry", parts_centre: bool = False,
            detection: Detection | None = None) -> list[FiberAxis]:
    """Axes of the given fibers, or of every fiber found in the image."""
    if not circles:
        circles = find_fibers(image)
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
            assert err < 2 and f.fiber_type == "Panda" and f.method == method, (true_angle, method, f)
    print("fiber_axis OK")
