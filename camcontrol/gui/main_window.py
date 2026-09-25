"""Main window: live image in the middle, controls panel on the right.

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
"""

import os
import time
from collections import deque
from pathlib import Path

import cv2
import numpy as np
from PySide6.QtCore import QSettings, Qt
from PySide6.QtGui import QAction, QKeySequence
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
from camcontrol.capture import CAPTURE_DIR, HD2_SIZE, NATIVE_SIZE
from camcontrol.gui.camera_worker import CameraWorker
from camcontrol.gui.image_view import ImageView

AVERAGE_CHOICES = [1, 4, 8, 16, 32]
SAVE_SIZES = [
    ("1920 x 1080 (native)", NATIVE_SIZE),
    ("3264 x 1836 (HD2 size, upscaled)", HD2_SIZE),
]
FORMATS = [("TIFF", "tif"), ("PNG", "png")]


def load_image_file(path: str) -> np.ndarray | None:
    """Read an image for display as 8-bit BGR or grayscale.

    Uses imdecode on the raw bytes, because cv2.imread can't open paths
    with non-ASCII characters on Windows.
    """
    data = np.fromfile(path, dtype=np.uint8)
    img = cv2.imdecode(data, cv2.IMREAD_UNCHANGED)
    if img is None:
        return None
    if img.dtype == np.uint16:
        img = (img >> 8).astype(np.uint8)  # 16-bit -> 8-bit for display
    elif img.dtype != np.uint8:
        img = cv2.normalize(img, None, 0, 255, cv2.NORM_MINMAX).astype(np.uint8)
    if img.ndim == 3 and img.shape[2] == 4:
        img = cv2.cvtColor(img, cv2.COLOR_BGRA2BGR)
    return img


class MainWindow(QMainWindow):
    def __init__(self, camera_index: int = 0):
        super().__init__()
        self.settings = QSettings("CamControl", "CamControl")
        self.live = True
        self._frame_times: deque[float] = deque(maxlen=30)

        self.view = ImageView()
        self.setCentralWidget(self.view)

        self._build_actions()
        self._build_menus()
        self._build_controls()
        self._build_status_bar()
        self._load_settings()
        self._set_title()

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

        self.exp_up_action = self._action("Exposure longer", "E", lambda: self._step_exposure(+1))
        self.exp_down_action = self._action("Exposure shorter", "D", lambda: self._step_exposure(-1))

    def _build_menus(self):
        m = self.menuBar().addMenu("&File")
        m.addActions([self.open_action, self.capture_action, self.open_folder_action])
        m.addSeparator()
        m.addAction(self.quit_action)

        m = self.menuBar().addMenu("&View")
        m.addActions([self.live_action, self.grid_action, self.crosshair_action])
        m.addSeparator()
        m.addActions([self.fit_action, self.actual_action, self.zoom_in_action, self.zoom_out_action])

        m = self.menuBar().addMenu("&Camera")
        m.addActions([self.exp_up_action, self.exp_down_action])

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
        self.defaults: dict[str, int] = {}
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
        defaults_button = QPushButton("Defaults")
        defaults_button.setToolTip("Reset every control to the camera's default value.")
        defaults_button.clicked.connect(self._reset_controls)
        form.addRow(defaults_button)
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

        dock = QDockWidget("Controls", self)
        dock.setObjectName("controls_dock")  # needed for saveState()
        dock.setWidget(panel)
        dock.setFeatures(QDockWidget.DockWidgetFeature.DockWidgetMovable
                         | QDockWidget.DockWidgetFeature.DockWidgetFloatable)
        self.addDockWidget(Qt.DockWidgetArea.RightDockWidgetArea, dock)

    def _build_status_bar(self):
        self.fps_label = QLabel()
        self.size_label = QLabel()
        self.zoom_label = QLabel()
        self.cursor_label = QLabel()
        self.cursor_label.setMinimumWidth(260)
        for w in (self.cursor_label, self.zoom_label, self.size_label, self.fps_label):
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
        else:
            self.resize(1400, 850)

    def _save_settings(self):
        s = self.settings
        s.setValue("capture/folder", self.folder_edit.text())
        s.setValue("capture/average_idx", self.average_combo.currentIndex())
        s.setValue("capture/size_idx", self.size_combo.currentIndex())
        s.setValue("capture/format_idx", self.format_combo.currentIndex())
        s.setValue("view/grid", self.grid_action.isChecked())
        s.setValue("view/crosshair", self.crosshair_action.isChecked())
        s.setValue("window/geometry", self.saveGeometry())
        s.setValue("window/state", self.saveState())

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
            self.defaults[name] = r["default"]
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

    def _reset_controls(self):
        for name, value in self.defaults.items():
            self.sliders[name].setValue(value)

    def _step_exposure(self, step: int):
        if self.cam_box.isEnabled():
            slider = self.sliders["exposure"]
            slider.setValue(slider.value() + step)

    def _set_live(self, on: bool):
        self.live = on
        self.live_action.setChecked(on)
        if not on:
            self.fps_label.setText("frozen")
            self._frame_times.clear()
        self._set_title()

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
        )

    def _choose_folder(self):
        folder = QFileDialog.getExistingDirectory(self, "Capture folder", self.folder_edit.text())
        if folder:
            self.folder_edit.setText(folder)

    def open_capture_folder(self):
        folder = Path(self.folder_edit.text())
        folder.mkdir(parents=True, exist_ok=True)
        os.startfile(folder)

    def open_image(self):
        path, _ = QFileDialog.getOpenFileName(
            self, "Open image", self.folder_edit.text(),
            "Images (*.tif *.tiff *.png *.jpg *.jpeg *.bmp);;All files (*)",
        )
        if not path:
            return
        img = load_image_file(path)
        if img is None:
            QMessageBox.warning(self, "CamControl", f"Could not read {path}")
            return
        self._set_live(False)  # stop the camera feed replacing it
        self.view.set_image(img)
        h, w = img.shape[:2]
        self.size_label.setText(f"{w} x {h}")
        self.fps_label.setText("file")
        self._set_title(Path(path).name)

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

    def closeEvent(self, event):
        self._save_settings()
        self.worker.stop()
        super().closeEvent(event)
