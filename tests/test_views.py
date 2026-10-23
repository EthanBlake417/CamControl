"""The views in the middle of the window, with fake cameras (see fakes.py)."""

import json
from pathlib import Path

import cv2
import numpy as np
from PySide6.QtCore import QSettings, Qt
from PySide6.QtTest import QTest
from PySide6.QtWidgets import QFileDialog

import fakes
from camcontrol.camera import EXPOSURE_US
from camcontrol.image_io import read_image_details
from conftest import ROOT, run_for, run_until

A, B = ("FakeCam A", 0), ("FakeCam B", 0)


def cam(pane):
    return pane.camera.key if pane.camera is not None else None


def two_cameras(w, qapp):
    p2 = w.add_view()
    w._set_pane_camera(p2, B)
    assert run_until(qapp, lambda: p2.camera.open and p2.view.image is not None)
    return w.panes[0], p2


def test_starts_with_one_live_view(window, qapp):
    w = window
    assert len(w.panes) == 1 and cam(w.panes[0]) == A and w.panes[0].live
    assert w.views.layout_name == "single" and not w.views.bar.isVisible()
    assert run_until(qapp, lambda: w.panes[0].view.image is not None)
    assert w.camera_status.text() == "Connected" and w.capture_action.isEnabled()


def test_two_cameras_live_at_once(window, qapp):
    w = window
    p1, p2 = two_cameras(w, qapp)
    assert w.views.layout_name != "single" and w.views.bar.isVisible()
    sent = [wk.frames_sent for wk in fakes.WORKERS]
    run_for(qapp, 0.5)
    assert all(after > before for after, before in zip([wk.frames_sent for wk in fakes.WORKERS], sent))
    assert p1.live and p2.live


def test_controls_follow_the_active_view(window, qapp):
    w = window
    p1, p2 = two_cameras(w, qapp)
    assert w.controlled is p2 and w.cam_box.title() == "Camera (view 2)"
    w.set_active(p1)
    assert w.controlled is p1 and w.camera_combo.currentText() == "FakeCam A"
    w.sliders["gain"].setValue(42)
    run_for(qapp, 0.3)
    by_index = {wk.index: wk.controls for wk in fakes.WORKERS}
    assert by_index[0] == {"gain": 42} and by_index[1] == {}
    # A view showing a file, without a camera: Controls stay on the last camera.
    p3 = w.add_view()
    w.open_image_path(str(ROOT / "Fiber Types.png"), p3)
    assert w.active is p3 and w.controlled is p1


def test_a_camera_moves_between_views(window, qapp):
    w = window
    p1, p2 = two_cameras(w, qapp)
    w._set_pane_camera(p1, B)  # B was in view 2
    run_for(qapp, 0.3)
    assert cam(p1) == B and p1.live and cam(p2) is None and not p2.live
    assert sum(wk.isRunning() for wk in fakes.WORKERS) == 1  # A closed, B reopened in view 1


def test_capture_linked_and_not(window, qapp, tmp_path):
    w = window
    two_cameras(w, qapp)
    w.views.set_linked(False)
    w.capture()
    assert run_until(qapp, lambda: not w._capturing)
    assert sum(len(wk.captures) for wk in fakes.WORKERS) == 1
    w.views.set_linked(True)
    w.name_edit.setText("sample")
    w.capture()
    assert run_until(qapp, lambda: not w._capturing)
    names = sorted(Path(c).name for wk in fakes.WORKERS for c in wk.captures)
    assert "sample_view1-001.tif" in names and "sample_view2-001.tif" in names
    assert all(Path(c).resolve().is_relative_to(tmp_path.resolve()) for wk in fakes.WORKERS for c in wk.captures)


def test_live_freeze_active_or_all(window, qapp):
    w = window
    p1, p2 = two_cameras(w, qapp)
    w.set_active(p1)
    w.views.set_linked(False)
    w._toggle_live(False)
    assert not p1.live and p2.live
    w._toggle_live(True)
    w.views.set_linked(True)
    w._toggle_live(False)
    assert not p1.live and not p2.live


def test_file_in_a_camera_view_keeps_the_camera(window, qapp):
    w = window
    p1 = w.panes[0]
    w.open_image_path(str(ROOT / "Fiber Types.png"))
    assert not p1.live and cam(p1) == A and w._source_text(p1)[0] == "Fiber Types.png"
    w._toggle_live(True)
    assert p1.live and p1.path is None


def test_remove_view_closes_its_camera(window, qapp):
    w = window
    p1, p2 = two_cameras(w, qapp)
    w.remove_view(p2)
    run_for(qapp, 0.2)
    assert w.panes == [p1] and w.views.layout_name == "single" and not w.views.bar.isVisible()
    assert sum(wk.isRunning() for wk in fakes.WORKERS) == 1
    w.remove_view(p1)  # the last view stays
    assert w.panes == [p1]


def test_layouts_and_single_tabs(window, qapp):
    w = window
    p1, p2 = two_cameras(w, qapp)
    for name in ("row", "column", "grid", "single"):
        w._set_layout(name)
        assert w.views.layout_name == name
    assert [b.text()[0] for b in w.views.tab_group.buttons()] == ["1", "2"]
    shown = [c.isVisible() for c in w.views.cells]
    assert shown.count(True) == 1 and shown[w.panes.index(w.active)]
    w.views.tab_clicked.emit(p1.view)
    assert w.active is p1 and w.views.cells[0].isVisible() and not w.views.cells[1].isVisible()


def test_measurements_belong_to_their_view(window, qapp):
    w = window
    p1, p2 = two_cameras(w, qapp)
    w.set_active(p1)
    w.measure_panel.add("line", [(10, 10), (100, 10)])
    w.set_active(p2)
    w.measure_panel.add("line", [(0, 0), (0, 50)])
    w.measure_panel.add("line", [(0, 0), (50, 0)])
    w.set_active(p1)
    assert [len(p.measurements) for p in w.panes] == [1, 2]
    assert len(w.measure_panel.measurements) == 1


def test_fiber_live_on_two_cameras(window, qapp):
    """Update live measures every live view (taking turns), each to its own truth."""
    w = window
    p1, p2 = two_cameras(w, qapp)
    w.fiber_panel.live_check.setChecked(True)

    def axes(p):
        return w.fiber_panel.axes if p is w.active else p.fiber_axes
    assert run_until(qapp, lambda: axes(p1) and axes(p2), 20)
    for p, index in ((p1, 0), (p2, 1)):
        truth = fakes.frame_for(index)[1]["angle"]
        assert abs(((axes(p)[0].slow_deg - truth) + 90) % 180 - 90) < 1.5


def test_result_and_save(window, qapp, tmp_path, monkeypatch):
    w = window
    w.show_result(np.full((100, 100, 3), 128, np.uint8), "Test result", "test", {"processing": "Test"})
    p = w.active
    assert p.result is not None and w._source_text(p)[0] == "Test result (unsaved)"
    target = tmp_path / "saved.tif"
    monkeypatch.setattr(QFileDialog, "getSaveFileName", lambda *a, **k: (str(target), ""))
    w.save_image()
    # The details go inside the image (no .json beside it), and show in the status bar.
    assert target.is_file() and not target.with_suffix(".json").exists()
    assert read_image_details(target)["processing"] == "Test"
    assert p.result is None and p.path == str(target)
    assert p.details is not None and "Test" in p.details[1]


def test_video_in_a_view(window, qapp, tmp_path):
    w = window
    clip = tmp_path / "clip.avi"
    out = cv2.VideoWriter(str(clip), cv2.VideoWriter_fourcc(*"MJPG"), 10, (320, 240))
    for i in range(20):
        out.write(np.full((240, 320, 3), i * 10, np.uint8))
    out.release()
    p1 = w.panes[0]
    p2 = w.add_view()
    w.open_image_path(str(clip), p2)
    assert w._video_pane is p2 and w.player.isVisible() and p2.view.image.shape == (240, 320, 3)
    assert p1.live  # the other view keeps its camera
    w._close_video_and_resume()
    assert w._video_pane is None and not w.player.isVisible()


def test_flat_field_reaches_every_camera(window, qapp, tmp_path):
    w = window
    two_cameras(w, qapp)
    flat = tmp_path / "flat.tif"
    cv2.imwrite(str(flat), np.full((64, 64, 3), 200, np.uint8))
    assert w._use_flat(str(flat))
    w._set_flat_live(True)
    assert all(type(wk.flat).__name__ == "FlatField" for wk in fakes.WORKERS if wk.isRunning())
    w._set_flat_live(False)
    assert all(wk.flat is None for wk in fakes.WORKERS if wk.isRunning())


def test_views_come_back_next_time(window_factory, qapp):
    w = window_factory(save_on_close=True)
    _, p2 = two_cameras(w, qapp)
    p3 = w.add_view()
    w.open_image_path(str(ROOT / "Fiber Types.png"), p3)
    w._set_layout("row")
    w.views.set_linked(False)
    w.close()
    w2 = window_factory()
    assert [cam(p) for p in w2.panes] == [A, B, None]
    assert w2.panes[2].path and Path(w2.panes[2].path).name == "Fiber Types.png"
    assert w2.views.layout_name == "row" and not w2.views.link_check.isChecked()


def test_settings_from_before_views(window_factory, tmp_path):
    """Saved settings that hold just camera/name become one view with that camera."""
    s = QSettings(str(tmp_path / "settings.ini"), QSettings.Format.IniFormat)
    s.setValue("camera/name", "FakeCam B")
    s.setValue("camera/n", 0)
    s.sync()
    w = window_factory()
    assert [cam(p) for p in w.panes] == [B]


def test_settings_files(window, qapp):
    w = window
    p1, p2 = two_cameras(w, qapp)
    w.set_active(p1)
    w.sliders["gain"].setValue(33)
    run_for(qapp, 0.3)
    data = json.loads(json.dumps(w.current_settings()))
    assert data["views"]["views"] == [{"camera": list(A)}, {"camera": list(B)}]
    assert data["cameras"]["FakeCam A"]["gain"] == 33
    # Values go to the camera they belong to.
    data["cameras"]["FakeCam B"] = {"gain": 55}
    w.apply_settings(data)
    run_for(qapp, 0.3)
    assert {wk.index: wk.controls.get("gain") for wk in fakes.WORKERS if wk.isRunning()} == {0: 33, 1: 55}
    # A file from before views: its camera values go to the camera Controls are on.
    w.apply_settings({"camcontrol_settings": 1, "camera": {"gain": 77}})
    run_for(qapp, 0.3)
    assert next(wk for wk in fakes.WORKERS if wk.index == 0 and wk.isRunning()).controls["gain"] == 77
    assert len(w.panes) == 2
    # A one-view file: the extra view (nothing on it) goes.
    w.apply_settings({"camcontrol_settings": 1, "views": {"layout": "single", "views": [{"camera": list(A)}]}})
    assert len(w.panes) == 1 and w.views.layout_name == "single"


def test_grid_and_crosshair_on_every_view(window, qapp):
    w = window
    two_cameras(w, qapp)
    w.show_overlay_settings("crosshair")
    tab = w._overlay_dialog.crosshair_tab
    tab.anchor.setCurrentIndex(tab.anchor.findData("window"))
    w.show_overlay_settings("grid")
    w._overlay_dialog.grid_tab.columns.setValue(5)
    p3 = w.add_view()  # joins in
    for p in w.panes:
        assert p.view.show_grid and p.view.show_crosshair
        assert p.view.grid_style.columns == 5 and p.view.crosshair_style.anchor == "window"
    assert p3.view.grid_style == w.grid_style


def test_right_click_shows_the_view_menu(window, qapp):
    from PySide6.QtCore import QPoint
    w = window
    p1, p2 = two_cameras(w, qapp)
    w.set_active(p1)
    shown = []
    w.view_menu.exec = lambda *a: shown.append(a)  # (PySide: patching the class doesn't take)
    QTest.mouseClick(p2.view.viewport(), Qt.MouseButton.RightButton, pos=QPoint(50, 50))
    assert len(shown) == 1 and w.active is p2, "the menu acts on the view clicked"
    assert w.overlay_settings_action in w.view_menu.actions()


def test_remembered_between_sessions(window_factory, qapp):
    w = window_factory(save_on_close=True)
    f = w.fiber_panel
    f.live_check.setChecked(True)
    f.use_circles.setChecked(False)
    f.min_rod_spin.setValue(40)
    w.fiber_log_panel.record_check.setChecked(True)
    w.histogram_panel.log_check.setChecked(True)
    w.count_panel._name_edits[1].setText("Dust")
    w.views.multi_layout = "column"
    w.show_overlay_settings("fiber")
    tab = w._overlay_dialog.fiber_tab
    tab.outside.setChecked(True)
    tab.show_axes.setCurrentIndex(tab.show_axes.findData("slow"))
    w.close()
    w2 = window_factory()
    f2 = w2.fiber_panel
    assert f2.live_check.isChecked() and not f2.use_circles.isChecked() and f2.detection().min_rod_px == 40
    assert w2.fiber_log_panel.record_check.isChecked() and w2.histogram_panel.log_check.isChecked()
    assert w2.count_panel.counter.names[1] == "Dust" and w2.views.multi_layout == "column"
    assert w2.fiber_outside and w2.fiber_lines.show_axes == "slow"
    assert w2.panes[0].view.fiber_drawing.long_lines and not w2.panes[0].view.fiber_drawing.show_fast
    w2.show_overlay_settings()
    assert w2._overlay_dialog.current_tab() == "fiber"


def test_fiber_settings_in_settings_files(window_factory, qapp):
    w = window_factory()
    f = w.fiber_panel
    f.set_method("core")
    f.centre_only_check.setChecked(True)
    f.min_rod_spin.setValue(25)
    w.fiber_outside = True
    w.fiber_lines.slow_color = "#123456"
    data = json.loads(json.dumps(w.current_settings()))
    fiber = data["fiber"]
    assert fiber["method"] == "core" and fiber["centre_only"] and fiber["outside_lines"]
    assert fiber["detection"]["min_rod_px"] == 25 and fiber["lines"]["slow_color"] == "#123456"
    w2 = window_factory()
    w2.apply_settings(data)
    assert w2.fiber_panel.method() == "core" and w2.fiber_panel.centre_only_check.isChecked()
    assert w2.fiber_panel.detection().min_rod_px == 25 and w2.fiber_outside
    assert w2.fiber_lines.slow_color == "#123456"
    # Unknown or bad values are left out.
    w2.apply_settings({"camcontrol_settings": 1, "fiber": {"method": "nonsense", "detection": {"smoothing": "x", "new": 1}}})
    assert w2.fiber_panel.method() == "core" and w2.fiber_panel.detection().smoothing == 1.0


def test_smallest_rod_drawn_on_the_view(window, qapp):
    w = window
    w.fiber_panel.advanced_check.setChecked(True)
    w.fiber_panel.draw_rod_button.setChecked(True)
    assert all(p.view.tool == "circle_centre" for p in w.panes)
    w.active.view.measurement_drawn.emit("circle_centre", [(100.0, 100.0), (130.0, 140.0)])
    assert w.fiber_panel.detection().min_rod_px == 50
    assert not w.fiber_panel.draw_rod_button.isChecked() and w.active.view.tool is None
    assert not w.measure_panel.measurements, "not kept as a measurement"
    # Esc cancels.
    w.fiber_panel.draw_rod_button.setChecked(True)
    w.active.view.tool_exit_requested.emit()
    assert not w.fiber_panel.draw_rod_button.isChecked() and not w._picking_rod


def test_fiber_log_and_crosshair_on_the_fiber(window, qapp):
    w = window
    assert run_until(qapp, lambda: w.active.view.image is not None)
    w.fiber_log_panel.record_check.setChecked(True)
    w.find_fiber_axes()
    assert run_until(qapp, lambda: w.fiber_panel.axes, 20)
    assert run_until(qapp, lambda: w.fiber_log_panel.log.entries, 5)
    assert w.fiber_log_panel.log.entries[0].fiber == 1
    view = w.active.view
    w.crosshair_style.anchor = "fiber"
    w.crosshair_action.setChecked(True)
    w._apply_overlays()
    assert view._crosshair_fiber() is w.fiber_panel.axes[0]
    view.grab()  # draws: readout, crosshair on the fiber


def test_fiber_axes_go_to_a_recording(window, qapp):
    w = window
    assert run_until(qapp, lambda: w.active.view.image is not None)
    pane = w.active
    sent = []
    pane.camera.worker.set_record_overlay = sent.append
    pane.camera.recording = True
    w.find_fiber_axes()
    assert run_until(qapp, lambda: sent and sent[-1] is not None, 20)
    axes, drawing = sent[-1]
    assert axes == w.fiber_panel.axes
    w.record_panel.draw_analysis.setChecked(False)
    w._sync_record_overlay(pane)
    assert sent[-1] is None


def test_exposure_set_as_a_time(window, qapp):
    w = window
    assert run_until(qapp, lambda: w.exposure_ms_box.isVisibleTo(w))
    assert not w.sliders["exposure"].isVisibleTo(w)
    assert abs(w.exposure_ms_spin.value() - 156.25) < 0.01
    w.exposure_ms_spin.setValue(300)
    run_for(qapp, 0.3)
    worker = next(wk for wk in fakes.WORKERS if wk.isRunning())
    assert worker.controls[EXPOSURE_US] == 300000
    data = json.loads(json.dumps(w.current_settings()))
    assert data["cameras"]["FakeCam A"][EXPOSURE_US] == 300000
