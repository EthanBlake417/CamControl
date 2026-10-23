"""Smaller parts: capture file names, settings files, overlay styles, the Captures panel,
fiber drawing and readout, the Fiber log and Histogram panels, details inside images."""

import json
import os
import time

import cv2
import numpy as np

from conftest import run_until
from camcontrol import settings_file
from camcontrol.capture import next_capture_path
from camcontrol.gui.overlay_settings import CrosshairStyle, GridStyle


def test_capture_names_continue_the_series(tmp_path):
    assert next_capture_path(tmp_path, "sample", "tif").name == "sample-001.tif"
    (tmp_path / "sample-001.tif").write_bytes(b"")
    (tmp_path / "sample-007.png").write_bytes(b"")  # any extension counts
    assert next_capture_path(tmp_path, "sample", "tif").name == "sample-008.tif"
    blank = next_capture_path(tmp_path, "", "tif").name
    assert blank.startswith("cap_") and blank.endswith(".tif")


def test_settings_file_round_trip(tmp_path):
    path = tmp_path / "job.json"
    settings_file.save_settings_file(path, {"output": {"name": "x"}})
    data = settings_file.load_settings_file(path)
    assert data["output"] == {"name": "x"} and data["camcontrol_settings"] == 1
    recent = settings_file.add_recent([str(tmp_path / "a.json")], path)
    assert recent[0] == str(path.resolve())


def test_overlay_styles_from_saved_settings():
    g = GridStyle.from_dict({"columns": 6, "square": True, "color": "nonsense", "opacity": 500, "unknown": 1})
    assert g.columns == 6 and g.square and g.color == GridStyle().color and g.opacity == 100
    assert g.rows_for(1920, 1080) == round(6 * 1080 / 1920)
    c = CrosshairStyle.from_dict({"anchor": "window", "circle": -5, "line": "dash"})
    assert c.anchor == "window" and c.circle == 0 and c.line == "dash"
    # Settings from before the crosshair could be set (grid only) still load.
    old = GridStyle.from_dict({"columns": 6, "rows": 8, "square": True, "width": 2.5, "color": "#00ffff",
                               "opacity": 60, "line": "dash"})
    assert old.width == 2.5 and old.color == "#00ffff"


def test_captures_panel(qapp, tmp_path):
    """Thumbnails load in the background; the open image is marked."""
    from camcontrol.gui.gallery import GalleryPanel
    for i in range(3):
        cv2.imwrite(str(tmp_path / f"img-{i:03d}.png"), np.full((300, 400, 3), 60 * i, np.uint8))
        time.sleep(0.01)  # distinct times: newest first
    g = GalleryPanel()
    g.set_folder(tmp_path)
    assert [g.list.item(i).text() for i in range(g.list.count())] == ["img-002.png", "img-001.png", "img-000.png"]
    assert run_until(qapp, lambda: all(key in g._cache for key in g._shown), 10)
    g.set_open_path(str(tmp_path / "img-001.png"))
    assert g.is_open(os.path.join(str(tmp_path), "IMG-001.PNG"))  # Windows paths: any case
    assert not g.is_open(str(tmp_path / "img-000.png"))


def test_fiber_line_styles_from_saved_settings():
    from camcontrol.gui.overlay_settings import AngleColors, FiberLineStyle
    f = FiberLineStyle.from_dict({"slow_color": "#0000ff", "fast_color": "red", "fast_opacity": 50.0})
    assert f.slow_color == "#0000ff" and f.fast_color == FiberLineStyle().fast_color and f.fast_opacity == 50
    # On by default: the slow axis aiming at 90, the fast at 0.
    assert f.slow_by_angle.bands(100)[0] == 90 and f.fast_by_angle.bands(100)[0] == 0
    saved = FiberLineStyle(fast_by_angle=AngleColors(True, 0.0, [3.0, 0.2, float("nan")], ["#010203", "bad", "#ffffff"]),
                           slow_by_angle=AngleColors(False, 90.0)).to_dict()
    back = FiberLineStyle.from_dict(json.loads(json.dumps(saved)))
    assert back.fast_by_angle.limits == [3.0, 0.2, 2.0]
    aim, bands = back.fast_by_angle.bands(50)
    assert aim == 0 and [b[0] for b in bands] == [0.2, 2.0, 3.0]
    assert bands[0][1].name() == "#ffff00" and bands[0][1].alpha() == 128  # the bad color back to its default
    assert back.slow_by_angle.bands(100) is None, "turned off stays off"
    assert FiberLineStyle.from_dict({"show_axes": "fast"}).show_axes == "fast"
    assert FiberLineStyle.from_dict({"show_axes": "sideways"}).show_axes == "both"
    # Saved before aims (off, no aim): the new defaults.
    old = FiberLineStyle.from_dict({"slow_by_angle": {"on": False, "limits": [1, 2, 3]}})
    assert old.slow_by_angle == AngleColors.for_axis(True)
    assert CrosshairStyle.from_dict({"anchor": "fiber"}).anchor == "fiber"


def test_axis_colors_by_angle_and_readout():
    from PySide6.QtGui import QColor
    from camcontrol.gui.fiber_panel import (FAST_COLOR, SLOW_COLOR, FiberDrawing, fiber_readout_lines,
                                            fiber_readout_pieces)
    from camcontrol.processing.fiber_axis import FiberAxis
    green, yellow, orange = QColor(0, 255, 0), QColor(255, 255, 0), QColor(255, 128, 0)
    bands = [(0.5, green), (1.0, yellow), (2.0, orange)]
    d = FiberDrawing(slow_bands=(0.0, bands))

    def at(deg):
        return FiberAxis(0, 0, 10, deg, 2, "Panda", 2, quality=3, coverage=1)
    assert d.slow_color(at(0.4)) == green and d.slow_color(at(179.6)) == green  # -0.4
    assert d.slow_color(at(1.0)) == yellow and d.slow_color(at(178.5)) == orange  # -1.5
    assert d.slow_color(at(5.0)) == SLOW_COLOR and d.fast_color(at(0.4)) == FAST_COLOR
    pieces = fiber_readout_pieces([at(0.4)], d)
    assert pieces[0][1][1] == green and pieces[0][0][1] == green, "the number and slow angle in its color"
    only_fast = FiberDrawing(slow_bands=(0.0, bands), show_slow=False)
    assert "".join(t for t, _ in fiber_readout_pieces([at(0.4)], only_fast)[0]) == "1: fast -89.6°"
    assert fiber_readout_lines([at(0.4)]) == ["1: slow +0.4°  fast -89.6°"]
    assert fiber_readout_lines([FiberAxis(0, 0, 10, 0.0, 1.0, "?", 0)])[0].endswith("(unclear)")
    aim90 = FiberDrawing(slow_bands=(90.0, bands))
    assert aim90.slow_color(at(90.4)) == green and aim90.slow_color(at(89.6)) == green
    assert aim90.slow_color(at(88.5)) == orange and aim90.slow_color(at(0.2)) == SLOW_COLOR


def test_axis_lines_outside_the_fiber(qapp):
    """Lines outside the fiber: none inside its circle or its neighbour's, out to the picture's edges."""
    from PySide6.QtGui import QColor, QImage
    from camcontrol.gui.fiber_panel import FiberDrawing, render_fiber_axes
    from camcontrol.processing.fiber_axis import FiberAxis
    f = FiberAxis(300, 200, 100, 0.0, 2, "Panda", 2, axis_x=310, axis_y=200)
    neighbour = FiberAxis(520, 200, 50, 45.0, 2, "Panda", 2, axis_x=520, axis_y=200)
    drawing = FiberDrawing(long_lines=True, slow=QColor(0, 0, 255), show_fast=False)
    out = render_fiber_axes(np.zeros((400, 600, 3), np.uint8), [f, neighbour], drawing)
    out = out.convertToFormat(QImage.Format.Format_RGB32)

    def blue(x, y):
        c = out.pixelColor(x, y)
        return c.blue() > 200 and c.red() < 60
    assert not any(blue(x, 200) for x in range(205, 396)), "slow axis line inside the fiber"
    assert not any(blue(x, 200) for x in range(475, 566)), "slow axis line across the neighbour"
    assert blue(410, 200) and blue(598, 200) and blue(190, 200) and blue(1, 200)
    assert blue(520 + 60, 200 - 60) and not blue(520 + 20, 200 - 20)  # the neighbour's own axis (45 deg)


def test_fiber_log_stats_and_save(qapp, tmp_path):
    import pandas as pd
    from camcontrol.gui.fiber_log import FiberLog, FiberLogPanel, axial_stats
    from camcontrol.processing.fiber_axis import FiberAxis
    m, s = axial_stats([89.0, -89.0])
    assert abs(abs(m) - 90) < 1e-9 and abs(s - 2 ** 0.5) < 1e-9
    m, s = axial_stats([1.0, 2.0, 3.0])
    assert abs(m - 2) < 1e-9 and abs(s - 1) < 1e-9
    assert axial_stats([5.0]) == (5.0, 0.0)
    panel = FiberLogPanel()
    fibers = [FiberAxis(100, 0, 10, 1.0, 2, "Panda", 2), FiberAxis(300, 0, 10, 179.0, 2, "Panda", 2)]
    panel.record(fibers)
    assert not panel.log.entries, "only with Record each update on"
    panel.record_check.setChecked(True)
    panel.record(fibers)
    panel.record(fibers)
    assert panel.log_table.rowCount() == 4 and panel.summary_table.rowCount() == 2
    second = panel.log.summary()[1]
    assert second["count"] == 2 and abs(second["mean_slow"] + 1) < 1e-9 and abs(second["mean_fast"] - 89) < 1e-9
    panel.log.save(tmp_path / "log.csv")
    assert (tmp_path / "log.csv").is_file() and (tmp_path / "log_summary.csv").is_file()
    panel.log.save(tmp_path / "log.xlsx")
    assert set(pd.read_excel(tmp_path / "log.xlsx", sheet_name=None)) == {"Fiber log", "Summary"}
    panel.clear()
    assert panel.log_table.rowCount() == 0 and FiberLog().updates == 0


def test_histogram_counts():
    from camcontrol.gui.histogram import count, log_percent, nice_ceiling, percents, stats, table
    img = np.zeros((10, 10, 3), np.uint8)
    img[0] = 255
    img[1, 0] = (0, 0, 255)  # red (BGR)
    c = count(img)
    p = percents(c[0])
    assert (p[255], p[76], p[0]) == (10.0, 1.0, 89.0)
    assert percents(c[1])[255] == 11.0
    assert log_percent(p[255]) == 1.0 and np.isnan(log_percent(p[1]))
    assert stats(c[0])[1:] == (0, 255)
    assert table(c).shape == (256, 9)
    assert nice_ceiling(37) == 50


def test_details_inside_saved_images(tmp_path):
    from camcontrol.capture import save_capture
    from camcontrol.image_io import details_summary, load_image_file, read_image_details, save_image_file
    img = (np.random.default_rng(0).random((40, 60, 3)) * 255).astype(np.uint8)
    meta = {"timestamp": "2026-10-23T10:00:00", "exposure_seconds_approx": 0.0156, "gain": 30,
            "frames_averaged": 4, "source": "µm test"}
    for ext in ("tif", "png", "jpg"):
        path = tmp_path / f"a µ.{ext}"
        save_image_file(path, img, meta)
        assert load_image_file(path).shape == img.shape
        assert read_image_details(path) == meta, ext
    assert details_summary(meta) == ("2026-10-23 10:00:00  ·  exposure 15.6 ms  ·  gain 30  ·  "
                                     "4 frames averaged  ·  from µm test")
    # Captures: no .json beside them any more; older ones' .json still read.
    path = save_capture(img, exposure=-6, gain=10, frames_averaged=2, folder=tmp_path, name="cap", exposure_s=0.5)
    assert not path.with_suffix(".json").exists() and read_image_details(path)["exposure_seconds_approx"] == 0.5
    old = tmp_path / "old.png"
    cv2.imwrite(str(old), img)
    old.with_suffix(".json").write_text(json.dumps({"gain": 7}))
    assert read_image_details(old) == {"gain": 7}


def test_exposure_times_between_and_past_the_steps():
    from camcontrol.camera import plan_exposure
    assert plan_exposure(625_000, -13, -3) == (-4, 1, 1.0)  # exactly step -4
    v, n, k = plan_exposure(900_000, -13, -3)  # step -4 (625 ms) brightened 1.44x
    assert (v, n) == (-4, 1) and abs(k - 1.44) < 1e-9
    v, n, k = plan_exposure(3_000_000, -13, -3)  # past the longest step (1.25 s): 3 frames at 0.8
    assert (v, n) == (-3, 3) and abs(k - 0.8) < 1e-9


def test_fiber_axes_drawn_on_recorded_frames(qapp):
    from camcontrol.gui.camera_worker import CameraWorker
    from camcontrol.gui.fiber_panel import FiberDrawing
    from camcontrol.processing.fiber_axis import FiberAxis
    worker = CameraWorker(None)
    frame = np.zeros((300, 400, 3), np.uint8)
    worker._overlay = ([FiberAxis(200, 150, 100, 0.0, 2, "Panda", 2)], FiberDrawing())
    drawn = worker._draw_overlay(frame)
    assert drawn.shape == frame.shape and drawn.flags["C_CONTIGUOUS"] and not frame.any()
    magenta = (drawn[..., 2] > 200) & (drawn[..., 0] > 150) & (drawn[..., 1] < 80)  # BGR
    assert magenta[145:156, 120:280].any(), "the slow axis"
    assert (drawn[5:40, 5:60] > 0).any(), "the readout in the top-left corner"
