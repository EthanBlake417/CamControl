"""Main window: live image in the middle, controls panel on the right,
measurements panel at the bottom.

Shortcuts:
    Space       capture
    E / D       exposure longer / shorter
    L           live / freeze
    G           grid
    C           crosshair
    F           fit image to window
    1           100% (one image pixel per screen pixel)
    Ctrl+= / -  zoom in / out (or use the mouse wheel)
    Ctrl+O      open an image file
    Ctrl+E      export measurements
    Ctrl+1/2/3  show / hide the Controls / Measurements / Captures panel
"""

import os
import time
from collections import deque
from pathlib import Path

from PySide6.QtCore import QSettings, Qt
from PySide6.QtGui import QAction, QKeySequence, QPixmap
from PySide6.QtWidgets import (
    QComboBox,
    QDockWidget,
    QFileDialog,
    QFormLayout,
    QGroupBox,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QMainWindow,
    QMessageBox,
    QPushButton,
    QSizePolicy,
    QSlider,
    QToolButton,
    QVBoxLayout,
    QWidget,
)

from camcontrol.camera import ADJUSTABLE, format_exposure
from camcontrol.capture import CAPTURE_DIR, HD2_SIZE, NATIVE_SIZE, clean_name, next_capture_path
from camcontrol.export import export_table, measurement_rows
from camcontrol.gui.camera_worker import CameraWorker
from camcontrol.gui.gallery import GalleryPanel
from camcontrol.gui.image_view import ImageView
from camcontrol.gui.measure_draw import render_annotated
from camcontrol.gui.measure_panel import MeasurePanel
from camcontrol.gui.qt_image import to_qimage
from camcontrol.image_io import load_image_file
from camcontrol.paths import LOGO

AVERAGE_CHOICES = [1, 4, 8, 16, 32]
SAVE_SIZES = [
    ("1920 x 1080 (native)", NATIVE_SIZE),
    ("3264 x 1836 (HD2 size, upscaled)", HD2_SIZE),
]
FORMATS = [("TIFF", "tif"), ("PNG", "png")]


class MainWindow(QMainWindow):
    def __init__(self, camera_index: int = 0):
        super().__init__()
        self.settings = QSettings("CamControl", "CamControl")
        self.live = True
        self._source = "live"  # what the view shows: "live" or an image file name
        self._frame_times: deque[float] = deque(maxlen=30)

        self.view = ImageView()
        self.setCentralWidget(self.view)
        self.measure_panel = MeasurePanel()
        self.gallery = GalleryPanel()

        self._build_actions()
        self._build_menus()
        self._build_controls()
        self._build_measure_dock()
        self._build_gallery_dock()
        self._build_status_bar()
        self._load_settings()
        self._set_title()
        self._show_source()

        self.worker = CameraWorker(camera_index)
        self.worker.opened.connect(self._on_camera_opened)
        self.worker.frame_ready.connect(self._on_frame)
        self.worker.settings_changed.connect(self._on_camera_settings)
        self.worker.capture_started.connect(self._on_capture_started)
        self.worker.capture_done.connect(self._on_capture_done)
        self.worker.error.connect(self._on_error)
        self.worker.start()

    # --- building the UI ----------------------------------------------------

    def _action(self, text, shortcut=None, slot=None, checkable=False):
        action = QAction(text, self)
        if shortcut:
            action.setShortcut(QKeySequence(shortcut))
        action.setCheckable(checkable)
        if slot:
            action.triggered.connect(slot)
        self.addAction(action)  # so the shortcut works anywhere in the window
        return action

    def _build_actions(self):
        self.open_action = self._action("&Open image...", "Ctrl+O", self.open_image)
        self.capture_action = self._action("&Capture", "Space", self.capture)
        self.capture_action.setEnabled(False)  # until the camera opens
        self.open_folder_action = self._action("Open captures &folder", None, self.open_capture_folder)
        self.quit_action = self._action("&Quit", "Ctrl+Q", self.close)

        self.live_action = self._action("&Live", "L", self._set_live, checkable=True)
        self.live_action.setChecked(True)
        self.grid_action = self._action("&Grid", "G", self.view.set_show_grid, checkable=True)
        self.crosshair_action = self._action("C&rosshair", "C", self.view.set_show_crosshair, checkable=True)
        self.fit_action = self._action("&Fit to window", "F", self.view.fit)
        self.actual_action = self._action("&100%", "1", self.view.actual_size)
        self.zoom_in_action = self._action("Zoom &in", "Ctrl+=", self.view.zoom_in)
        self.zoom_out_action = self._action("Zoom &out", "Ctrl+-", self.view.zoom_out)

        self.export_action = self._action("&Export measurements...", "Ctrl+E", self.export_measurements)
        self.clear_measurements_action = self._action(
            "&Clear measurements", None, lambda: self.measure_panel.clear())

    def _build_menus(self):
        m = self.menuBar().addMenu("&File")
        m.addActions([self.open_action, self.capture_action, self.open_folder_action])
        m.addSeparator()
        m.addAction(self.quit_action)

        m = self.menuBar().addMenu("&View")
        m.addActions([self.live_action, self.grid_action, self.crosshair_action])
        m.addSeparator()
        m.addActions([self.fit_action, self.actual_action, self.zoom_in_action, self.zoom_out_action])
        m.addSeparator()
        # Filled in by _add_dock(), one entry per panel.
        self.panels_menu = m.addMenu("&Panels")

        m = self.menuBar().addMenu("&Measure")
        m.addActions([self.export_action, self.clear_measurements_action])

        m = self.menuBar().addMenu("&Help")
        m.addAction(self._action("&About CamControl", None, self.show_about))

    def _build_measure_dock(self):
        panel = self.measure_panel
        view = self.view
        view.set_measurements(panel.measurements, panel.calibration)

        panel.tool_changed.connect(view.set_tool)
        panel.selection_changed.connect(view.set_selected)
        panel.changed.connect(
            lambda: view.set_measurements(panel.measurements, panel.calibration, panel.selected_id()))
        panel.export_requested.connect(self.export_measurements)
        view.measurement_drawn.connect(self._on_measurement_drawn)
        view.tool_exit_requested.connect(lambda: panel.set_tool(None))

        self._add_dock("Measurements", "measure_dock", panel,
                       Qt.DockWidgetArea.BottomDockWidgetArea, "Ctrl+2")

    def _build_gallery_dock(self):
        self.gallery.open_requested.connect(self.open_image_path)
        self.gallery.folder_changed.connect(self._update_next_name)
        self.gallery_dock = self._add_dock("Captures", "gallery_dock", self.gallery,
                                           Qt.DockWidgetArea.RightDockWidgetArea, "Ctrl+3")
        self._place_gallery_under_controls()

    def _place_gallery_under_controls(self):
        """Default spot for the Captures panel: under Controls, wherever that is."""
        self.splitDockWidget(self.controls_dock, self.gallery_dock, Qt.Orientation.Vertical)

    def _add_dock(self, title, object_name, widget, area, shortcut):
        """Add a panel that can be moved, floated or closed.

        Closed panels come back from View > Panels (or the shortcut).
        Which panels are open is saved with the window layout.
        """
        dock = QDockWidget(title, self)
        dock.setObjectName(object_name)  # needed for saveState()
        dock.setWidget(widget)
        dock.setFeatures(QDockWidget.DockWidgetFeature.DockWidgetMovable
                         | QDockWidget.DockWidgetFeature.DockWidgetFloatable
                         | QDockWidget.DockWidgetFeature.DockWidgetClosable)
        self.addDockWidget(area, dock)
        # Qt's ready-made show/hide action: checked while the panel is visible.
        toggle = dock.toggleViewAction()
        toggle.setShortcut(QKeySequence(shortcut))
        self.addAction(toggle)  # shortcut works even while the panel is hidden
        self.panels_menu.addAction(toggle)
        return dock

    def _tool_button(self, action):
        """A button that mirrors an action (text, checked and enabled state)."""
        b = QToolButton()
        b.setDefaultAction(action)
        b.setToolButtonStyle(Qt.ToolButtonStyle.ToolButtonTextOnly)
        b.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        return b

    def _build_controls(self):
        panel = QWidget()
        layout = QVBoxLayout(panel)

        # Camera: one slider per control. Ranges are filled in when the
        # camera opens and reports them (_on_camera_opened).
        cam_box = QGroupBox("Camera")
        form = QFormLayout(cam_box)
        self.sliders: dict[str, QSlider] = {}
        self.value_labels: dict[str, QLabel] = {}
        for name in ADJUSTABLE:
            slider = QSlider(Qt.Orientation.Horizontal)
            slider.setPageStep(1)
            label = QLabel("-")
            label.setMinimumWidth(90)
            slider.valueChanged.connect(lambda v, n=name: self._on_slider(n, v))
            row = QHBoxLayout()
            row.addWidget(slider)
            row.addWidget(label)
            form.addRow(name.capitalize(), row)
            self.sliders[name] = slider
            self.value_labels[name] = label
        cam_box.setEnabled(False)  # until the camera opens
        self.cam_box = cam_box
        layout.addWidget(cam_box)

        # Capture
        cap_box = QGroupBox("Capture")
        form = QFormLayout(cap_box)
        self.average_combo = QComboBox()
        for n in AVERAGE_CHOICES:
            self.average_combo.addItem(f"{n} frame{'s' if n > 1 else ''}", n)
        self.average_combo.setToolTip("Averaging several frames reduces noise.")
        form.addRow("Average", self.average_combo)

        self.size_combo = QComboBox()
        for text, size in SAVE_SIZES:
            self.size_combo.addItem(text, size)
        self.size_combo.setToolTip(
            "The camera sends 1920x1080 at most. 3264x1836 is scaled up to match\n"
            "HD2 files: more pixels, no extra detail, and a different µm/px."
        )
        form.addRow("Save size", self.size_combo)

        self.format_combo = QComboBox()
        for text, ext in FORMATS:
            self.format_combo.addItem(text, ext)
        form.addRow("Format", self.format_combo)

        folder_row = QHBoxLayout()
        self.folder_edit = QLineEdit()
        self.folder_edit.setReadOnly(True)
        browse = QPushButton("...")
        browse.setFixedWidth(30)
        browse.clicked.connect(self._choose_folder)
        folder_row.addWidget(self.folder_edit)
        folder_row.addWidget(browse)
        form.addRow("Folder", folder_row)

        self.name_edit = QLineEdit()
        self.name_edit.setPlaceholderText("blank = date and time")
        self.name_edit.setToolTip(
            "Files are saved as name-001, name-002, ... continuing after the\n"
            "highest number already in the folder, so nothing is overwritten."
        )
        form.addRow("Name", self.name_edit)
        self.next_name_label = QLabel()
        self.next_name_label.setStyleSheet("color: gray;")
        form.addRow("", self.next_name_label)
        # Keep the "Next:" preview current.
        self.name_edit.textChanged.connect(self._update_next_name)
        self.format_combo.currentIndexChanged.connect(self._update_next_name)

        capture_button = self._tool_button(self.capture_action)
        capture_button.setMinimumHeight(40)
        form.addRow(capture_button)
        self.last_saved_label = QLabel("")
        self.last_saved_label.setWordWrap(True)
        form.addRow(self.last_saved_label)
        layout.addWidget(cap_box)

        # View
        view_box = QGroupBox("View")
        grid = QVBoxLayout(view_box)
        row1 = QHBoxLayout()
        for a in (self.live_action, self.grid_action, self.crosshair_action):
            row1.addWidget(self._tool_button(a))
        row2 = QHBoxLayout()
        for a in (self.fit_action, self.actual_action):
            row2.addWidget(self._tool_button(a))
        grid.addLayout(row1)
        grid.addLayout(row2)
        layout.addWidget(view_box)

        layout.addStretch()

        self.controls_dock = self._add_dock("Controls", "controls_dock", panel,
                                            Qt.DockWidgetArea.RightDockWidgetArea, "Ctrl+1")

    def _build_status_bar(self):
        self.fps_label = QLabel()
        self.size_label = QLabel()
        self.zoom_label = QLabel()
        self.cursor_label = QLabel()
        self.cursor_label.setMinimumWidth(260)
        # What the view is showing: live, frozen, or which file.
        self.source_label = QLabel()
        self.source_label.setStyleSheet("font-weight: bold")
        for w in (self.cursor_label, self.zoom_label, self.size_label, self.fps_label, self.source_label):
            self.statusBar().addPermanentWidget(w)
        self.view.zoom_changed.connect(lambda z: self.zoom_label.setText(f"{z * 100:.0f}%"))
        self.view.cursor_moved.connect(self._on_cursor)

    # --- settings ---------------------------------------------------------------

    def _load_settings(self):
        s = self.settings
        self.folder_edit.setText(s.value("capture/folder", str(CAPTURE_DIR)))
        self.average_combo.setCurrentIndex(int(s.value("capture/average_idx", 0)))
        self.size_combo.setCurrentIndex(int(s.value("capture/size_idx", 0)))
        self.format_combo.setCurrentIndex(int(s.value("capture/format_idx", 0)))
        self.name_edit.setText(s.value("capture/name", ""))
        self.gallery.set_count(int(s.value("gallery/count", 12)))
        self.gallery.set_folder(self.folder_edit.text())
        self._update_next_name()
        # Set the actions, then the view (setChecked doesn't fire triggered).
        for action, key, setter in (
            (self.grid_action, "view/grid", self.view.set_show_grid),
            (self.crosshair_action, "view/crosshair", self.view.set_show_crosshair),
        ):
            on = s.value(key, False, type=bool)
            action.setChecked(on)
            setter(on)
        if s.contains("window/geometry"):
            self.restoreGeometry(s.value("window/geometry"))
            self.restoreState(s.value("window/state"))
            # A layout saved before the Captures panel existed doesn't know
            # where it goes, so put it under Controls (which may have moved).
            if not s.value("window/has_gallery", False, type=bool):
                self._place_gallery_under_controls()
        else:
            self.resize(1400, 850)

    def _save_settings(self):
        s = self.settings
        s.setValue("capture/folder", self.folder_edit.text())
        s.setValue("capture/average_idx", self.average_combo.currentIndex())
        s.setValue("capture/size_idx", self.size_combo.currentIndex())
        s.setValue("capture/format_idx", self.format_combo.currentIndex())
        s.setValue("capture/name", self.name_edit.text())
        s.setValue("gallery/count", self.gallery.count)
        s.setValue("view/grid", self.grid_action.isChecked())
        s.setValue("view/crosshair", self.crosshair_action.isChecked())
        s.setValue("window/geometry", self.saveGeometry())
        s.setValue("window/state", self.saveState())
        s.setValue("window/has_gallery", True)

    # --- camera events ------------------------------------------------------------

    def _on_camera_opened(self, ranges: dict):
        for name, slider in self.sliders.items():
            r = ranges.get(name)
            if r is None:
                slider.setEnabled(False)
                continue
            slider.blockSignals(True)
            slider.setRange(r["min"], r["max"])
            slider.blockSignals(False)
        self.cam_box.setEnabled(True)
        self.capture_action.setEnabled(True)
        self.statusBar().showMessage("Camera open", 3000)

    def _on_frame(self, frame):
        # Always acknowledge, so the worker sends the next frame.
        self.worker.frame_consumed()
        if not self.live:
            return
        self.view.set_image(frame)

        now = time.perf_counter()
        self._frame_times.append(now)
        if len(self._frame_times) > 1:
            span = self._frame_times[-1] - self._frame_times[0]
            if span > 0:
                self.fps_label.setText(f"{(len(self._frame_times) - 1) / span:.1f} fps")
        h, w = frame.shape[:2]
        self.size_label.setText(f"{w} x {h}")

    def _on_camera_settings(self, values: dict):
        for name, value in values.items():
            slider = self.sliders.get(name)
            if slider is None:
                continue
            # Don't fight the user while they're dragging a slider.
            if not slider.isSliderDown():
                slider.blockSignals(True)
                slider.setValue(value)
                slider.blockSignals(False)
            self._show_value(name, value)

    def _on_capture_started(self, n_frames: int, seconds: float):
        self.capture_action.setEnabled(False)
        self.statusBar().showMessage(f"Capturing {n_frames} frame(s), about {seconds:.1f} s...")

    def _on_capture_done(self, path: str):
        self.capture_action.setEnabled(True)
        self.last_saved_label.setText(f"Saved {Path(path).name}")
        self.statusBar().showMessage(f"Saved {path}", 5000)
        self._update_next_name()
        # The folder watcher would pick it up too; this also selects it.
        self.gallery.refresh(select=path)

    def _on_error(self, message: str):
        # Capture stays disabled if the camera never opened.
        self.capture_action.setEnabled(self.cam_box.isEnabled())
        self.statusBar().showMessage(message)
        QMessageBox.warning(self, "CamControl", message)

    # --- user actions ---------------------------------------------------------------

    def _show_value(self, name: str, value: int):
        text = f"{value}"
        if name == "exposure":
            text += f"  (~{format_exposure(value)})"
        self.value_labels[name].setText(text)

    def _on_slider(self, name: str, value: int):
        self._show_value(name, value)
        self.worker.set_control(name, value)

    def _set_live(self, on: bool):
        self.live = on
        self.live_action.setChecked(on)
        if on:
            self._source = "live"
        else:
            self.fps_label.clear()
            self._frame_times.clear()
        self._set_title()
        self._show_source()

    def _show_source(self, path: str | None = None):
        if path:
            self.source_label.setText(Path(path).name)
            self.source_label.setToolTip(str(path))
        else:
            self.source_label.setText("Live" if self.live else "Frozen")
            self.source_label.setToolTip("")

    def _set_title(self, file_name: str | None = None):
        if file_name:
            self.setWindowTitle(f"CamControl - {file_name}")
        else:
            self.setWindowTitle(f"CamControl - {'live' if self.live else 'frozen'}")

    def capture(self):
        self.worker.capture(
            self.average_combo.currentData(),
            self.size_combo.currentData(),
            self.format_combo.currentData(),
            Path(self.folder_edit.text()),
            clean_name(self.name_edit.text()),
        )

    def _update_next_name(self):
        path = next_capture_path(Path(self.folder_edit.text()), self.name_edit.text(),
                                 self.format_combo.currentData())
        if self.name_edit.text().strip():
            self.next_name_label.setText(f"Next: {path.name}")
        else:
            self.next_name_label.setText("Next: cap_<date>_<time>." + self.format_combo.currentData())

    def _choose_folder(self):
        folder = QFileDialog.getExistingDirectory(self, "Capture folder", self.folder_edit.text())
        if folder:
            self.folder_edit.setText(folder)
            self.gallery.set_folder(folder)
            self._update_next_name()

    def open_capture_folder(self):
        folder = Path(self.folder_edit.text())
        folder.mkdir(parents=True, exist_ok=True)
        os.startfile(folder)

    def open_image(self):
        path, _ = QFileDialog.getOpenFileName(
            self, "Open image", self.folder_edit.text(),
            "Images (*.tif *.tiff *.png *.jpg *.jpeg *.bmp);;All files (*)",
        )
        if path:
            self.open_image_path(path)

    def open_image_path(self, path: str):
        """Show an image file in the view (freezes the live feed)."""
        img = load_image_file(path)
        if img is None:
            QMessageBox.warning(self, "CamControl", f"Could not read {path}")
            return
        # Measurements belong to the image they were drawn on.
        if self.measure_panel.measurements and QMessageBox.question(
            self, "Open image", "Clear the current measurements? They were made on a different image."
        ) == QMessageBox.StandardButton.Yes:
            self.measure_panel.clear(confirm=False)
        self._set_live(False)  # stop the camera feed replacing it
        self.view.set_image(img)
        h, w = img.shape[:2]
        self.size_label.setText(f"{w} x {h}")
        self._source = Path(path).name
        self._set_title(self._source)
        self._show_source(path)

    # --- measurements ---------------------------------------------------------------

    def _on_measurement_drawn(self, kind: str, points: list):
        error = self.measure_panel.add(kind, points)
        if error:
            self.statusBar().showMessage(error, 5000)

    def export_measurements(self):
        """Save the table (Excel or CSV) plus the image and an annotated copy."""
        panel = self.measure_panel
        if not panel.measurements or self.view.image is None:
            QMessageBox.information(self, "Export", "There are no measurements to export.")
            return
        default = Path(self.folder_edit.text()) / f"measurements_{time.strftime('%Y%m%d_%H%M%S')}.xlsx"
        path, _ = QFileDialog.getSaveFileName(
            self, "Export measurements", str(default), "Excel (*.xlsx);;CSV (*.csv)")
        if not path:
            return
        path = Path(path)
        if path.suffix.lower() not in (".xlsx", ".csv"):
            path = path.with_suffix(".xlsx")
        path.parent.mkdir(parents=True, exist_ok=True)

        cal = panel.calibration
        image = self.view.image
        try:
            export_table(measurement_rows(panel.measurements, cal, self._source), path, cal, self._source)
            # The exact image that was measured, and a copy with the shapes drawn on.
            image_path = path.with_name(f"{path.stem}_image.png")
            annotated_path = path.with_name(f"{path.stem}_annotated.png")
            to_qimage(image).save(str(image_path))
            render_annotated(image, panel.measurements, cal).save(str(annotated_path))
        except Exception as e:  # e.g. the file is open in Excel
            QMessageBox.warning(self, "Export", f"Export failed: {e}")
            return
        self.statusBar().showMessage(
            f"Exported {len(panel.measurements)} measurements to {path.name}, "
            f"{image_path.name} and {annotated_path.name}", 8000)

    def _on_cursor(self, info):
        if info is None:
            self.cursor_label.setText("")
            return
        x, y, value = info
        if isinstance(value, tuple):
            text = f"RGB {value[0]} {value[1]} {value[2]}"
        else:
            text = f"value {value}"
        self.cursor_label.setText(f"x {x}, y {y}   {text}")

    def show_about(self):
        box = QMessageBox(self)
        box.setWindowTitle("About CamControl")
        logo = QPixmap(str(LOGO))
        if not logo.isNull():
            box.setIconPixmap(logo.scaledToWidth(160, Qt.TransformationMode.SmoothTransformation))
        box.setText(
            "<b>CamControl</b><br>"
            "Live view, capture and measurement for the O.C. White Ultra-Cam II."
        )
        box.exec()

    def closeEvent(self, event):
        self._save_settings()
        self.worker.stop()
        super().closeEvent(event)
