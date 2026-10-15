"""Smaller parts: capture file names, settings files, overlay styles, the Captures panel."""

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
