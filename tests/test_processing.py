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
