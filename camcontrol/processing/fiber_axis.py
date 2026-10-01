"""Axis of symmetry of polarization-maintaining (PM) fiber end faces.

PM fibers have stress parts on either side of the core: two round rods
(Panda), two wedges (Bow-tie), or an elliptical region round the core
(Elliptical-clad). The line through them is the fiber's slow axis; the
fast axis is at 90 degrees. Both are mirror-symmetry axes of the end face.

How it's measured, for each fiber:
1. Find the fiber: the round cladding (Hough circle detection), or a
   circle given by the caller (e.g. a Circle measurement).
2. Find the stress parts: inside the cladding, the pixels whose color is
   clearly different from the cladding's usual color (Otsu threshold on the
   color distance). The core, in the middle, and the edge of the cladding
   are left out.
3. Their principal axis (second moments about the fiber centre) is the slow
   axis. How much longer the spread is along it than across it
   ("elongation") says how clear the result is; near 1 means no clear axis.
4. Type guess from the stress parts' shape: one region round the core
   (Elliptical), two round blobs (Panda), two other shapes (Bow-tie).

Angles are in degrees, counter-clockwise from horizontal as seen on screen
(image y points down, so it's flipped), from 0 to 180.

Run directly to test on an image (default: "Fiber Types.png"):
    python -m camcontrol.processing.fiber_axis [image]
"""

import math
import sys
from dataclasses import dataclass
from pathlib import Path

import cv2
import numpy as np

from camcontrol.processing.common import as_bgr

CORE_FRACTION = 0.12   # inner part of the radius left out (the core)
EDGE_FRACTION = 0.92   # outer part left out (the cladding edge)
MIN_ELONGATION = 1.3   # below this the axis isn't clear


@dataclass
class FiberAxis:
    cx: float
    cy: float
    radius: float
    slow_deg: float        # angle of the slow axis (through the stress parts)
    elongation: float      # spread along / across the axis; >= 1
    fiber_type: str        # "Panda", "Bow-tie", "Elliptical" or "?"
    n_parts: int           # stress regions found

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


def measure_axis(image: np.ndarray, cx: float, cy: float, radius: float) -> FiberAxis:
    """The slow axis of the fiber with cladding circle (cx, cy, radius)."""
    img = as_bgr(image)
    h, w = img.shape[:2]
    x0, x1 = max(0, int(cx - radius)), min(w, int(cx + radius) + 1)
    y0, y1 = max(0, int(cy - radius)), min(h, int(cy + radius) + 1)
    crop = img[y0:y1, x0:x1]
    yy, xx = np.mgrid[y0:y1, x0:x1]
    dx, dy = xx - cx, yy - cy
    dist = np.hypot(dx, dy)
    ring = (dist > radius * CORE_FRACTION) & (dist < radius * EDGE_FRACTION)
    if ring.sum() < 50:
        return FiberAxis(cx, cy, radius, 0.0, 1.0, "?", 0)

    # Color distance from the cladding's usual color (Lab: closer to how we see it).
    lab = cv2.cvtColor(crop, cv2.COLOR_BGR2LAB).astype(np.float32)
    cladding = np.median(lab[ring], axis=0)
    diff = np.linalg.norm(lab - cladding, axis=2)
    diff = cv2.GaussianBlur(diff, (0, 0), max(1.0, radius / 100))
    values = np.clip(diff[ring], 0, 255).astype(np.uint8)
    threshold, _ = cv2.threshold(values.reshape(-1, 1), 0, 255, cv2.THRESH_BINARY + cv2.THRESH_OTSU)
    mask = ring & (diff > max(threshold, 8))  # 8: ignore noise when there's no real contrast

    # Principal axis of the stress parts about the fiber centre.
    wx, wy = dx[mask], -dy[mask]  # y flipped: angles as seen on screen
    if len(wx) < 20:
        return FiberAxis(cx, cy, radius, 0.0, 1.0, "?", 0)
    mu20, mu02, mu11 = (wx * wx).mean(), (wy * wy).mean(), (wx * wy).mean()
    angle = 0.5 * math.degrees(math.atan2(2 * mu11, mu20 - mu02)) % 180
    spread = math.sqrt(((mu20 - mu02) / 2) ** 2 + mu11 ** 2)
    along, across = (mu20 + mu02) / 2 + spread, (mu20 + mu02) / 2 - spread
    elongation = math.sqrt(along / max(across, 1e-9))

    n_parts, fiber_type = _classify(mask.astype(np.uint8), radius)
    return FiberAxis(cx, cy, radius, angle, elongation, fiber_type, n_parts)


def _classify(mask: np.ndarray, radius: float) -> tuple[int, str]:
    mask = cv2.morphologyEx(mask, cv2.MORPH_OPEN, np.ones((3, 3), np.uint8))
    contours, _ = cv2.findContours(mask, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
    min_area = (radius * 0.08) ** 2  # ignore specks
    parts = [c for c in contours if cv2.contourArea(c) >= min_area]
    if len(parts) == 1:
        return 1, "Elliptical"
    if len(parts) == 2:
        # Panda rods are round; bow-tie wedges aren't.
        roundness = [4 * math.pi * cv2.contourArea(c) / max(cv2.arcLength(c, True) ** 2, 1e-9) for c in parts]
        return 2, "Panda" if min(roundness) > 0.8 else "Bow-tie"
    return len(parts), "?"


def analyse(image: np.ndarray, circles: list[tuple[float, float, float]] | None = None) -> list[FiberAxis]:
    """Axes of the given fibers, or of every fiber found in the image."""
    if not circles:
        circles = find_fibers(image)
    return [measure_axis(image, *c) for c in circles]


if __name__ == "__main__":
    from camcontrol.image_io import load_image_file

    path = sys.argv[1] if len(sys.argv) > 1 else str(Path(__file__).resolve().parents[2] / "Fiber Types.png")
    img = load_image_file(path)
    for f in sorted(analyse(img), key=lambda f: f.cx):
        print(f"fiber at ({f.cx:.0f}, {f.cy:.0f}) r {f.radius:.0f}: {f.fiber_type:10} slow axis {f.slow_deg:6.1f} deg, "
              f"fast {f.fast_deg:6.1f} deg, elongation {f.elongation:.2f}, parts {f.n_parts}")

    # Self-test: a drawn Panda fiber rotated to known angles.
    for true_angle in (0, 30, 75, 120):
        test = np.full((400, 400, 3), 30, np.uint8)
        cv2.circle(test, (200, 200), 150, (240, 220, 180), -1)
        a = math.radians(true_angle)
        for s in (-1, 1):
            cv2.circle(test, (round(200 + s * 75 * math.cos(a)), round(200 - s * 75 * math.sin(a))), 35, (200, 120, 30), -1)
        cv2.circle(test, (200, 200), 8, (60, 220, 240), -1)
        f = measure_axis(test, 200, 200, 150)
        err = min(abs(f.slow_deg - true_angle), 180 - abs(f.slow_deg - true_angle))
        assert err < 2 and f.fiber_type == "Panda", (true_angle, f)
    print("fiber_axis OK")
