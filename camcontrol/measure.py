"""Measurement geometry (no GUI code here).

A Measurement stores only what the user clicked: its kind and its points in
image pixels (x to the right, y down, sub-pixel floats). Results are
computed on demand for a given Calibration, so changing the calibration
later re-expresses every measurement without re-clicking.

Per-axis calibration: every point is first converted to physical units,
(x * cal.x, y * cal.y), and the geometry is done on those. That's what makes
non-square pixels come out right. For example, a circle that is round on
the sample would be an ellipse in pixels, and angles would be distorted.

Run directly for a quick self-test:
    python -m camcontrol.measure
"""

import math
from dataclasses import dataclass

from camcontrol.calibration import PIXELS, Calibration

Point = tuple[float, float]


@dataclass(frozen=True)
class Kind:
    name: str
    label: str
    n_points: int | None  # fixed number of clicks, or None = keep clicking until finished
    min_points: int


KINDS: dict[str, Kind] = {k.name: k for k in [
    Kind("line", "Line", 2, 2),
    Kind("polyline", "Polyline", None, 2),
    Kind("circle", "Circle (3 pt)", 3, 3),
    Kind("circle_centre", "Circle (centre)", 2, 2),  # centre, then a point on the edge
    Kind("angle", "Angle", 3, 3),
    Kind("rectangle", "Rectangle", 2, 2),
    Kind("polygon", "Polygon", None, 3),
]}

CIRCLE_KINDS = ("circle", "circle_centre")


@dataclass
class Measurement:
    id: int
    kind: str
    points: list[Point]


# --- basic geometry -------------------------------------------------------------

def distance(a: Point, b: Point) -> float:
    return math.hypot(b[0] - a[0], b[1] - a[1])


def path_length(points: list[Point], closed: bool = False) -> float:
    total = sum(distance(points[i], points[i + 1]) for i in range(len(points) - 1))
    if closed and len(points) > 2:
        total += distance(points[-1], points[0])
    return total


def polygon_area(points: list[Point]) -> float:
    """Shoelace formula. Works for any simple (non-self-crossing) polygon."""
    s = 0.0
    for i in range(len(points)):
        x1, y1 = points[i]
        x2, y2 = points[(i + 1) % len(points)]
        s += x1 * y2 - x2 * y1
    return abs(s) / 2


def circle_through(a: Point, b: Point, c: Point) -> tuple[float, float, float]:
    """Centre (x, y) and radius of the circle through three points."""
    (ax, ay), (bx, by), (cx, cy) = a, b, c
    d = 2 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by))
    # |d| is 4x the triangle's area. Compare it to the longest side squared
    # so the "too close to a straight line" test doesn't depend on scale.
    longest = max(distance(a, b), distance(b, c), distance(c, a))
    if longest == 0 or abs(d) < 1e-6 * longest ** 2:
        raise ValueError("the three points are (nearly) in a straight line")
    a2, b2, c2 = ax * ax + ay * ay, bx * bx + by * by, cx * cx + cy * cy
    ux = (a2 * (by - cy) + b2 * (cy - ay) + c2 * (ay - by)) / d
    uy = (a2 * (cx - bx) + b2 * (ax - cx) + c2 * (bx - ax)) / d
    return ux, uy, distance((ux, uy), a)


def angle_at(a: Point, vertex: Point, b: Point) -> float:
    """Angle a-vertex-b in degrees (0 to 180)."""
    v1 = (a[0] - vertex[0], a[1] - vertex[1])
    v2 = (b[0] - vertex[0], b[1] - vertex[1])
    if v1 == (0, 0) or v2 == (0, 0):
        raise ValueError("an angle arm has zero length")
    cross = v1[0] * v2[1] - v1[1] * v2[0]
    dot = v1[0] * v2[0] + v1[1] * v2[1]
    return math.degrees(math.atan2(abs(cross), dot))


# --- results --------------------------------------------------------------------

def compute(m: Measurement, cal: Calibration = PIXELS) -> dict:
    """Results for a measurement, in the calibration's units.

    Possible keys: length, perimeter, area, radius, diameter, width, height,
    angle_deg, and center_px (circle centre in image pixels, for drawing).
    Raises ValueError if the points don't make a valid shape.
    """
    kind = KINDS[m.kind]
    if len(m.points) < kind.min_points:
        raise ValueError(f"{kind.label} needs at least {kind.min_points} points")

    p = [(x * cal.x, y * cal.y) for x, y in m.points]

    if m.kind == "line":
        return {"length": distance(p[0], p[1])}

    if m.kind == "polyline":
        return {"length": path_length(p)}

    if m.kind in CIRCLE_KINDS:
        if m.kind == "circle":
            cx, cy, r = circle_through(p[0], p[1], p[2])
        else:
            (cx, cy), r = p[0], distance(p[0], p[1])
            if r == 0:
                raise ValueError("the circle has zero radius")
        return {
            "radius": r,
            "diameter": 2 * r,
            "perimeter": 2 * math.pi * r,
            "area": math.pi * r * r,
            "center_px": (cx / cal.x, cy / cal.y),
        }

    if m.kind == "angle":
        return {"angle_deg": angle_at(p[0], p[1], p[2])}

    if m.kind == "rectangle":
        w = abs(p[1][0] - p[0][0])
        h = abs(p[1][1] - p[0][1])
        return {"width": w, "height": h, "area": w * h, "perimeter": 2 * (w + h)}

    if m.kind == "polygon":
        return {"area": polygon_area(p), "perimeter": path_length(p, closed=True)}

    raise ValueError(f"unknown measurement kind {m.kind!r}")


def fmt(value: float) -> str:
    """Number formatting used in labels and the table."""
    return f"{value:.2f}"


def summary(m: Measurement, results: dict, cal: Calibration = PIXELS) -> str:
    """Short text for the on-image label, e.g. '123.45 px' or '90.00°'."""
    u = cal.unit
    if m.kind in ("line", "polyline"):
        return f"{fmt(results['length'])} {u}"
    if m.kind in CIRCLE_KINDS:
        return f"⌀ {fmt(results['diameter'])} {u}"
    if m.kind == "angle":
        return f"{fmt(results['angle_deg'])}°"
    if m.kind == "rectangle":
        return f"{fmt(results['width'])} × {fmt(results['height'])} {u}"
    if m.kind == "polygon":
        return f"A {fmt(results['area'])} {u}²"
    return ""


if __name__ == "__main__":
    def check(name, got, want):
        ok = abs(got - want) < 1e-9
        print(f"{'ok  ' if ok else 'FAIL'} {name}: {got:.6f} (want {want})")

    check("line 3-4-5", compute(Measurement(1, "line", [(0, 0), (3, 4)]))["length"], 5)
    check("polyline", compute(Measurement(1, "polyline", [(0, 0), (3, 4), (3, 10)]))["length"], 11)
    c = compute(Measurement(1, "circle", [(0, 10), (10, 0), (-10, 0)]))
    check("circle radius", c["radius"], 10)
    check("circle centre x", c["center_px"][0], 0)
    c = compute(Measurement(1, "circle_centre", [(5, 5), (8, 9)]))
    check("centre circle radius", c["radius"], 5)
    check("centre circle centre y", c["center_px"][1], 5)
    check("angle 90", compute(Measurement(1, "angle", [(10, 0), (0, 0), (0, 10)]))["angle_deg"], 90)
    check("angle 45", compute(Measurement(1, "angle", [(10, 0), (0, 0), (10, 10)]))["angle_deg"], 45)
    r = compute(Measurement(1, "rectangle", [(2, 3), (12, 8)]))
    check("rect area", r["area"], 50)
    check("rect perimeter", r["perimeter"], 30)
    sq = [(0, 0), (10, 0), (10, 10), (0, 10)]
    check("polygon area", compute(Measurement(1, "polygon", sq))["area"], 100)
    check("polygon perimeter", compute(Measurement(1, "polygon", sq))["perimeter"], 40)

    # Non-square pixels: 2 µm/px in X, 1 µm/px in Y.
    cal = Calibration("test", 2.0, 1.0, "µm")
    check("aniso horizontal line", compute(Measurement(1, "line", [(0, 0), (10, 0)]), cal)["length"], 20)
    check("aniso vertical line", compute(Measurement(1, "line", [(0, 0), (0, 10)]), cal)["length"], 10)
    # An ellipse in pixels (rx 5, ry 10) that is a circle of radius 10 µm on the sample.
    e = compute(Measurement(1, "circle", [(5, 0), (0, 10), (-5, 0)]), cal)
    check("aniso circle radius", e["radius"], 10)
    check("aniso angle", compute(Measurement(1, "angle", [(5, 0), (0, 0), (5, 10)]), cal)["angle_deg"], 45)

    try:
        compute(Measurement(1, "circle", [(0, 0), (5, 5), (10, 10)]))
        print("FAIL collinear circle accepted")
    except ValueError as err:
        print(f"ok   collinear circle rejected: {err}")
