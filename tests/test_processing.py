"""Image processing: the modules' own self-tests, and fiber axis accuracy."""

import math
import subprocess
import sys

import cv2
import numpy as np
import pytest

import synth
from conftest import ROOT
from camcontrol.image_io import load_image_file
from camcontrol.processing import fiber_axis as fa

# Modules whose "python -m" run is a self-test (asserts, or prints FAIL).
SELF_TESTS = [
    "camcontrol.measure",
    "camcontrol.recording",
    "camcontrol.processing.count",
    "camcontrol.processing.flatfield",
    "camcontrol.processing.fluorescence",
    "camcontrol.processing.focus_stack",
    "camcontrol.processing.hdr",
    "camcontrol.processing.stitch",
    "camcontrol.processing.fiber_axis",
]


@pytest.mark.parametrize("module", SELF_TESTS)
def test_module_self_test(module):
    run = subprocess.run([sys.executable, "-m", module], cwd=ROOT, capture_output=True, text=True, timeout=300,
                         env={**__import__("os").environ, "PYTHONIOENCODING": "utf-8"})
    assert run.returncode == 0, run.stdout[-2000:] + run.stderr[-2000:]
    assert "FAIL" not in run.stdout, run.stdout[-2000:]


def angle_error(measured_deg: float, true_deg: float) -> float:
    """Difference of two axis angles (degrees, 0-90: an axis has no direction)."""
    return abs(((measured_deg - true_deg) + 90) % 180 - 90)


def _drawn_panda(angle_deg: float, blob: bool = False):
    """The self-test's fibers: two touching fibers (the right one cut off),
    thin rod rings, and optionally a dark blurred blob over one rod."""
    test = np.full((500, 800, 3), 10, np.uint8)
    a = math.radians(angle_deg)
    for fx, lit in ((300, 230), (650, 120)):
        cv2.circle(test, (fx, 250), 175, (lit,) * 3, -1)
        for s in (-1, 1):
            cv2.circle(test, (round(fx + s * 85 * math.cos(a)), round(250 - s * 85 * math.sin(a))), 50,
                       (lit + 15,) * 3, 2)
    if blob:
        mask = np.zeros(test.shape[:2], np.float32)
        cv2.circle(mask, (round(300 - 85 * math.cos(a)), round(250 + 85 * math.sin(a))), 35, 80, -1)
        test = np.clip(test - cv2.GaussianBlur(mask, (0, 0), 12)[..., None], 0, 255).astype(np.uint8)
    return cv2.GaussianBlur(test, (0, 0), 1.5)


@pytest.mark.parametrize("angle", range(0, 180, 10))
def test_rods_with_dirt_over_one(angle):
    """A dark blob over one rod mustn't pull the axis (every angle, not just a lucky few)."""
    f = fa.analyse(_drawn_panda(angle, blob=True), method="rods", centre_only=True)[0]
    assert angle_error(f.slow_deg, angle) < 1.0
    assert f.clear


def test_fiber_types_drawing():
    """Fiber Types.png (in the repository): a drawn Panda, Bow-tie and Elliptical fiber, all horizontal."""
    found = sorted(fa.analyse(load_image_file(str(ROOT / "Fiber Types.png"))), key=lambda f: f.cx)
    assert [f.fiber_type for f in found] == ["Panda", "Bow-tie", "Elliptical"]
    for f in found:
        assert angle_error(f.slow_deg, 0) < 0.3
        assert f.clear


def test_cladding_edge_ignores_glow():
    """A soft glow past a dark gap round the fiber, brighter than half the
    cladding (as on the ETN3000), isn't taken for its edge."""
    img = np.zeros((900, 900), np.float32)
    glow = np.zeros_like(img)
    cv2.circle(glow, (450, 450), 345, 150, 40)                  # wide, gentle
    img = cv2.GaussianBlur(glow, (0, 0), 10)
    fiber = np.zeros_like(img)
    cv2.circle(fiber, (450, 450), 300, 200, -1)
    img = np.where(cv2.GaussianBlur(fiber, (0, 0), 1.5) > 1, cv2.GaussianBlur(fiber, (0, 0), 1.5), img)
    img = np.clip(img, 0, 255).astype(np.uint8)
    cx, cy, r = fa.find_fibers(cv2.cvtColor(img, cv2.COLOR_GRAY2BGR))[0]
    assert abs(r - 300) < 3 and math.hypot(cx - 450, cy - 450) < 2


@pytest.mark.parametrize("angle", [0, 35, 90, 140])
def test_big_rods_near_the_edge(angle):
    """Rods of 0.32 R whose outer edge reaches 0.90 R (the ETN3000 fibers reach about 0.86 R) are still found."""
    test = np.full((800, 800, 3), 15, np.uint8)
    cv2.circle(test, (400, 400), 300, (190,) * 3, -1)
    a = math.radians(angle)
    for s in (-1, 1):
        cv2.circle(test, (round(400 + s * 174 * math.cos(a)), round(400 - s * 174 * math.sin(a))), 96, (150,) * 3, -1)
    test = cv2.GaussianBlur(test, (0, 0), 1.5)
    f = fa.analyse(test, method="rods", centre_only=True)[0]
    assert angle_error(f.slow_deg, angle) < 0.5
    assert f.clear and abs(f.rod_radii[0] - 96) < 4


@pytest.mark.slow
@pytest.mark.parametrize("style", ["relief", "mixed"])
def test_synthetic_dirty_fibers(style):
    """Accuracy on synthetic dirty Panda fibers with known axes (40 of each kind).
    The limits are a little looser than what the code does today (2026-10-14:
    median 0.09 / 0.18 deg, 94-95% within 1 deg), so a real regression fails
    but small changes don't."""
    errors, clear_but_wrong = [], 0
    for seed in range(40):
        img, truth = synth.make(seed, 1.0, style)
        found = fa.analyse(img, centre_only=True)
        assert found, f"seed {seed}: no fiber found"
        f = found[0]
        e = angle_error(f.slow_deg, truth["angle"])
        errors.append(e)
        clear_but_wrong += f.clear and e > 5
    errors = np.array(errors)
    assert np.median(errors) < 0.3
    assert np.mean(errors < 1.0) >= 0.85
    assert clear_but_wrong == 0, "a result marked clear was more than 5 degrees off"


# The user's own photos (not in the repository): checked when they're there.
REAL = [("WIN_20261006_11_09_45_Pro.jpg", -6.0, 0.6), ("Fiber 3 WIN_20261006_11_13_59_Pro.jpg", 0.1, 0.5)]


@pytest.mark.parametrize("name,expected,tolerance", REAL)
def test_real_end_faces(name, expected, tolerance):
    path = ROOT / name
    if not path.is_file():
        pytest.skip(f"{name} isn't in the project folder")
    f = fa.analyse(load_image_file(str(path)), centre_only=True)[0]
    assert f.fiber_type == "Panda" and f.clear
    assert angle_error(f.slow_deg, expected) < tolerance


@pytest.mark.parametrize("name", ["36879B-00FBB-001.tif", "36879B-00FBB-008.tif"])
def test_real_bow_ties(name):
    path = ROOT / name
    if not path.is_file():
        pytest.skip(f"{name} isn't in the project folder")
    f = fa.analyse(load_image_file(str(path)), centre_only=True)[0]
    assert f.fiber_type == "Bow-tie"
    assert angle_error(f.slow_deg, 0.45) < 0.5


def test_plain_disc_does_not_crash():
    """A round fiber with no rods at all (no edges inside) is unclear, not an error."""
    img = np.full((600, 600, 3), 15, np.uint8)
    cv2.circle(img, (300, 300), 200, (190,) * 3, -1)
    f = fa.analyse(cv2.GaussianBlur(img, (0, 0), 1.5), centre_only=True)[0]
    assert not f.clear


@pytest.mark.parametrize("angle", [0, 8, 172])
def test_rod_running_off_the_image(angle):
    """A fiber at the frame's edge with a rod partly off the image: the rod is
    scored over the part of its outline shown, so it's found where it is."""
    test = np.full((500, 800, 3), 12, np.uint8)
    cv2.circle(test, (690, 250), 175, (200,) * 3, -1)
    a = math.radians(angle)
    for s in (-1, 1):
        cv2.circle(test, (round(690 + s * 95 * math.cos(a)), round(250 - s * 95 * math.sin(a))), 48, (150,) * 3, 3)
    f = fa.measure_axis(cv2.GaussianBlur(test, (0, 0), 1.5), 690, 250, 175, "rods")
    assert angle_error(f.slow_deg, angle) < 1.0
    right = max(x for x, _ in f.part_centres)
    assert abs(right - (690 + 95 * abs(math.cos(a)))) < 4


def test_real_vgroove_photo():
    """Three Panda fibers side by side, the outer rods running off the image, a dark
    blotch over one rod, the core showing as a bright spot (the user's photo)."""
    path = ROOT / "WIN_20261008_07_31_07_Pro.jpg"
    if not path.is_file():
        pytest.skip("WIN_20261008_07_31_07_Pro.jpg isn't in the project folder")
    found = sorted(fa.analyse(load_image_file(str(path))), key=lambda f: f.cx)
    assert len(found) == 3
    for f, truth in zip(found, (-2.0, -0.9, -0.3)):  # judged from the rod outlines by eye
        assert f.fiber_type == "Panda" and f.clear
        assert angle_error(f.slow_deg, truth) < 1.0


def _drawn_apple_core(slow_deg: float):
    """A drawn apple core: wide, pinched top and bottom where the rods press
    (slow axis through the pinch), its rods too faint to see."""
    c, r = 300.0, 250.0
    t = math.radians(slow_deg)
    sx, sy = math.cos(t), -math.sin(t)  # along the slow axis, y down
    img = np.full((600, 600, 3), 20, np.uint8)
    cv2.circle(img, (300, 300), 250, (150, 150, 150), -1)
    yy, xx = np.mgrid[:600, :600].astype(np.float64)
    px, py = xx - c, yy - c
    s, f = px * sx + py * sy, -px * sy + py * sx
    w, h = 0.17 * r, 0.09 * r + 0.05 * r * (f / (0.17 * r)) ** 2
    img[(np.abs(f) < w) & (np.abs(s) < h)] = 220
    return cv2.GaussianBlur(img, (0, 0), 1.5)


@pytest.mark.parametrize("truth", [0.0, 30.0, 93.0, 150.0])
def test_apple_core_gives_the_axes(truth):
    img = _drawn_apple_core(truth)
    core = fa.apple_core(img, 300, 300, 250)
    assert core.clear and not core.by_rods and angle_error(core.slow_deg, truth) < 1, core
    # Rod outlines beside it pick the slow axis, whichever way the pinches look;
    # rods far from both of its axes are ignored.
    for rods, slow in ((truth + 4, truth), (truth + 87, truth + 90), (truth + 45, truth)):
        c = fa.apple_core(img, 300, 300, 250, rods)
        assert angle_error(c.slow_deg, slow) < 1 and c.by_rods == (rods != truth + 45), (rods, c)
    # No rods to see: Automatic falls back on the apple core.
    f = fa.measure_axis(img, 300, 300, 250, "auto")
    assert f.method == "core" and f.clear and angle_error(f.slow_deg, truth) < 1, f


@pytest.mark.parametrize("lit_core", [False, True])
def test_finds_a_fiber_by_its_faint_edge(lit_core):
    """A grey fiber on a grey background, as when lit from the front: only a
    faint edge shows, with or without a lit core that stands out."""
    img = np.full((720, 1280), 150, np.int32)
    ring = np.zeros((720, 1280), np.uint8)
    cv2.circle(ring, (660, 370), 330, 1, 4)
    img[ring > 0] = 134
    for s in (-1, 1):
        cv2.circle(ring, (660, 370 + s * 165), 95, 2, -1)
    img[ring == 2] = 140
    if lit_core:
        core = np.zeros_like(ring)
        cv2.circle(core, (660, 370), 45, 1, -1)
        img[core > 0] = 255
    # Uneven light (brighter on the left, so a brightness threshold splits the image,
    # not the fiber) and a little sensor noise.
    img = img + 45 - (np.arange(1280) * 90 // 1280)[None, :] + np.random.default_rng(1).integers(-4, 5, img.shape)
    img = cv2.GaussianBlur(cv2.cvtColor(np.clip(img, 0, 255).astype(np.uint8), cv2.COLOR_GRAY2BGR), (0, 0), 1.5)
    found = fa.find_fibers(img)
    assert found, "nothing found"
    x, y, r = found[0]
    # The cladding (not the core, not nothing), roughly: a thin drawn ring isn't refined like a real edge.
    assert math.hypot(x - 660, y - 370) < 33 and abs(r - 330) < 33, found


def test_smallest_rod_leaves_out_small_rings():
    """Big rods at 30 deg, and a pair of smaller, crisper rings at 120 deg: with
    Smallest rod above the small rings' size, only the big rods count."""
    img = np.full((400, 400, 3), 20, np.uint8)
    cv2.circle(img, (200, 200), 150, (200, 200, 200), -1)
    for deg, gap, r, gray in ((30.0, 85.0, 50, 175), (120.0, 60.0, 28, 0)):
        a = math.radians(deg)
        for s in (-1, 1):
            cv2.circle(img, (round(200 + s * gap * math.cos(a)), round(200 - s * gap * math.sin(a))), r, (gray,) * 3, 3)
    img = cv2.GaussianBlur(img, (0, 0), 1.2)
    picky = fa.Detection(min_rod=30.0)
    f = fa.measure_axis(img, 200, 200, 150, "rods", detection=picky)
    assert angle_error(f.slow_deg, 30) < 1, f.slow_deg
    assert all(r >= 0.3 * 150 - 0.5 for r in f.rod_radii), f.rod_radii
    # The same in pixels, as drawn on the view: each fiber gets it as a share of its radius (18-40%).
    assert fa.Detection(min_rod_px=45).min_rod_for(150) == 30
    assert fa.Detection(min_rod_px=500).min_rod_for(150) == fa.MAX_MIN_ROD
    assert fa.Detection(min_rod_px=0, min_rod=25).min_rod_for(150) == 25


def test_brightest_centre_counted_from_the_right():
    img = np.full((300, 900, 3), 10, np.uint8)
    for x, core in ((150, 120), (450, 250), (750, 160)):
        cv2.circle(img, (x, 150), 130, (90, 90, 90), -1)
        cv2.circle(img, (x, 150), 6, (core,) * 3, -1)
    img[150, 150] = 255  # one hot pixel doesn't count
    axes = [fa.FiberAxis(x, 150, 130, 0, 1, "?", 0, centre_brightness=fa.centre_brightness(img, x, 150, 130))
            for x in (150, 450, 750)]
    b = [f.centre_brightness for f in axes]
    assert abs(b[1] - 250) < 1 and b[0] < 200, b
    assert fa.brightest_centre(axes) == (1, 2)
    axes[2].centre_brightness = 255
    assert fa.brightest_centre(axes) == (2, 1)
    assert fa.brightest_centre(axes[:1]) is None


def test_live_frames_follow_the_fibers():
    """Update live follows the fibers measured last time instead of searching again."""
    img = _drawn_panda(20)
    first = fa.analyse(img, method="rods")
    previous = [(f.cx, f.cy, f.radius) for f in first]
    followed = fa.track_fibers(img, previous)
    assert followed is not None and len(followed) == len(previous)
    assert all(math.hypot(a[0] - b[0], a[1] - b[1]) < 1 for a, b in zip(followed, previous))
    again = fa.analyse(img, method="rods", previous=previous)
    assert [round(f.slow_deg, 1) for f in again] == [round(f.slow_deg, 1) for f in first]
    assert fa.track_fibers(img, [(100, 100, 30)]) is None  # nothing there any more: search again


# The user's grey, front-lit captures of one Panda fiber (rods about vertical, often
# washed out; a sharp apple core), from 2026-10-20 (in the Rust app's captures folder).
# Measured within 4 deg of the rods' direction by eye. Left out: 180801 (rods unclear),
# 181639 (only the core lit), and 181407 (its faint edge fits to a slightly different
# circle than in the Rust app, and the result comes out unclear, though at the right angle).
GREY_CAPTURES = ["180148", "180150", "180430", "180445", "180452", "180703", "180708", "180712", "180746",
                 "180757", "181139", "181330", "181345", "181355", "181428", "181720"]


@pytest.mark.slow
@pytest.mark.parametrize("name", GREY_CAPTURES)
def test_real_grey_captures(name):
    path = ROOT / "camcontrol-rs" / "target" / "release" / "captures" / f"cap_20261020_{name}.tif"
    if not path.is_file():
        pytest.skip("the user's captures aren't here")
    found = fa.analyse(load_image_file(path))
    assert len(found) == 1
    assert found[0].clear and angle_error(found[0].slow_deg, 93) < 4, found[0]
