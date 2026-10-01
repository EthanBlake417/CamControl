"""Main window: live image in the middle, controls and captures panels on
the right, measurements and counting panels at the bottom.

Shortcuts:
    Space       capture
    L           live / freeze
    G           grid
    C           crosshair
    F           fit image to window
    1           100% (one image pixel per screen pixel)
    Ctrl+= / -  zoom in / out (or use the mouse wheel)
    Ctrl+O      open an image file
    Ctrl+S      save the image in the view (e.g. a processing result)
    Ctrl+E      export measurements
    Ctrl+K      side-by-side compare (right-click an image in Captures to pick it)
    Ctrl+1..5   show / hide the Controls / Measurements / Captures / Counting / Fiber axis panel

Processing tools (Process menu, or select images in Captures and right-click)
show their result in the view, unsaved, until you press Ctrl+S.
"""

import json
import os
import time
from collections import deque
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path

import pandas as pd
from PySide6.QtCore import QSettings, Qt, QTimer
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
    QScrollArea,
    QSizePolicy,
    QSlider,
    QTabWidget,
    QToolButton,
    QVBoxLayout,
    QWidget,
)

from camcontrol.camera import ADJUSTABLE, format_exposure
from camcontrol.capture import CAPTURE_DIR, HD2_SIZE, NATIVE_SIZE, clean_name, next_capture_path
from camcontrol.export import export_table, measurement_rows
from camcontrol import settings_file
from camcontrol.gui.camera_worker import CameraWorker
from camcontrol.gui.compare import CompareArea, side_by_side
from camcontrol.gui.count_panel import CountPanel, render_marks
from camcontrol.gui.fiber_panel import FiberPanel, render_fiber_axes
from camcontrol.gui.gallery import GalleryPanel
from camcontrol.gui.image_view import ImageView
from camcontrol.gui.jobs import run_job
from camcontrol.gui.measure_draw import render_annotated
from camcontrol.gui.measure_panel import MeasurePanel
from camcontrol.gui.process_dialogs import (
    FILE_FILTER,
    FluorescenceDialog,
    FocusStackDialog,
    HdrDialog,
    StitchDialog,
)
from camcontrol.gui.qt_image import to_qimage
from camcontrol.gui.record_panel import RecordPanel
from camcontrol.gui.tool_stripes import ToolStripes
from camcontrol.gui.video_player import VideoPlayer
from camcontrol.image_io import is_video, load_image_file, save_image_file
from camcontrol.paths import LOGO
from camcontrol.measure import CIRCLE_KINDS, compute
from camcontrol.processing.count import Counter
from camcontrol.processing.fiber_axis import analyse as analyse_fibers
from camcontrol.processing.flatfield import FlatField

AVERAGE_CHOICES = [1, 4, 8, 16, 32]
SAVE_SIZES = [
    ("1920 x 1080 (native)", NATIVE_SIZE),
    ("3264 x 1836 (HD2 size, upscaled)", HD2_SIZE),
]
FORMATS = [("TIFF", "tif"), ("PNG", "png")]
FLAT_DIR = CAPTURE_DIR.parent / "flats"  # flat-field references taken in the app
FIBER_LIVE_INTERVAL_S = 0.25  # how often "Update live" re-measures fiber axes
# Process menu / Captures right-click tools: key -> dialog class.
PROCESS_DIALOGS = {
    "stack": FocusStackDialog,
    "hdr": HdrDialog,
    "stitch": StitchDialog,
    "composite": FluorescenceDialog,
}


@dataclass
class Pane:
    """One image of the window (left = main view, right = compare view) and the
    measurements, counting marks and fiber axes made on it. The panels show
    whichever pane is active (the one clicked last)."""
    view: ImageView
    measurements: list
    counter: Counter
    next_id: int = 1
    fiber_axes: list = field(default_factory=list)


class MainWindow(QMainWindow):
    def __init__(self, camera_index: int = 0):
        super().__init__()
        self.settings = QSettings("CamControl", "CamControl")
        self.live = True
        self._source = "live"  # what the view shows: "live" or an image file name
        # Set while the view shows an unsaved processing result: what made it
        # (saved to the .json sidecar) and a suggested file name.
        self._result: dict | None = None
        self._frame_times: deque[float] = deque(maxlen=30)
        self.flat: FlatField | None = None
        self._flat_path: str | None = None
        self._dialogs = {}  # processing dialogs, kept so they remember their files and options
        self._job = None    # background processing job while one runs
        self._pending_camera: dict | None = None  # settings-file camera values waiting for the camera
        self._camera_values: dict = {}             # latest values the camera reported
        self.settings_path: str | None = None      # settings file opened or saved last

        self.camera_index = camera_index
        self.camera_open = False

        self._set_corners()
        # No Qt tab groups: each edge shows one panel at a time and the stripe
        # buttons act as its tabs. (Qt's tab bars also leave stray tabs drawn
        # in the corner when a tabbed panel is hidden.)
        self.setDockOptions(QMainWindow.DockOption.AnimatedDocks | QMainWindow.DockOption.AllowNestedDocks)
        # PyCharm-style stripes along the edges: a button per panel to minimize / restore it.
        self.stripes = ToolStripes(self)

        # The main view, with the compare view beside it (hidden until used).
        # Under it, the video player bar (shown while a video is open).
        self.view = ImageView()
        self.compare = CompareArea(self.view)
        self.player = VideoPlayer()
        self.player.hide()
        self.player.frame_ready.connect(self._on_video_frame)
        self.player.close_requested.connect(lambda: self._set_live(True))
        centre = QWidget()
        centre_layout = QVBoxLayout(centre)
        centre_layout.setContentsMargins(0, 0, 0, 0)
        centre_layout.setSpacing(0)
        centre_layout.addWidget(self.compare, stretch=1)
        centre_layout.addWidget(self.player)
        self.setCentralWidget(centre)
        self.measure_panel = MeasurePanel()
        self.count_panel = CountPanel()
        self.fiber_panel = FiberPanel()
        self._fiber_last = 0.0  # when the live view was last measured
        # Left image, right (compare) image; the panels start on the left one.
        self.panes = [
            Pane(self.view, self.measure_panel.measurements, self.count_panel.counter),
            Pane(self.compare.view, [], Counter(names=self.count_panel.counter.names)),
        ]
        self.active = 0
        self.gallery = GalleryPanel()
        self.record_panel = RecordPanel(self)  # the Video and Time-lapse tabs in Controls

        self._build_actions()
        self._build_menus()
        self._build_controls()
        self._build_measure_dock()
        self._build_count_dock()
        self._build_fiber_dock()
        self._build_gallery_dock()
        self._build_compare()
        self._build_status_bar()
        self._load_settings()
        self._set_title()
        self._show_source()

        self._start_worker()

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
        self.save_action = self._action("&Save image as...", "Ctrl+S", self.save_image)
        self.open_folder_action = self._action("Open captures &folder", None, self.open_capture_folder)
        self.reconnect_action = self._action("&Reconnect camera", None, self.reconnect_camera)
        self.open_settings_action = self._action("Open se&ttings...", None, self.open_settings)
        self.save_settings_action = self._action("Save setti&ngs", "Ctrl+Shift+S", self.save_settings)
        self.save_settings_as_action = self._action("Save settings &as...", None, self.save_settings_as)
        self.compare_action = self._action("&Compare side by side", "Ctrl+K", self._show_compare, checkable=True)
        self.open_compare_action = self._action("Open image to co&mpare...", None, self.open_compare)
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
        self.export_counts_action = self._action("Export &counts...", None, self.export_counts)
        self.find_fibers_action = self._action("Find &fiber axes", None, self.find_fiber_axes)
        self.export_fibers_action = self._action("Export fiber a&xes...", None, self.export_fiber_axes)

        self.process_actions = {
            key: self._action(dialog.title + "...", None, lambda _=False, k=key: self.run_process(k))
            for key, dialog in PROCESS_DIALOGS.items()
        }
        self.flat_info_action = self._action("No flat reference")
        self.flat_info_action.setEnabled(False)
        self.flat_set_action = self._action("&Use current image as flat reference", None,
                                            self.set_flat_from_view)
        self.flat_load_action = self._action("&Load flat reference...", None, self.load_flat)
        self.flat_live_action = self._action("&Correct live view and captures", None,
                                             self._set_flat_live, checkable=True)
        self.flat_apply_action = self._action("Correct the &image in the view", None,
                                              self.apply_flat_to_view)
        self._update_flat_actions()

    def _build_menus(self):
        m = self.menuBar().addMenu("&File")
        m.addActions([self.open_action, self.open_compare_action, self.save_action,
                      self.capture_action, self.open_folder_action])
        m.addSeparator()
        # Settings files (camera, capture, output, flat-field, recording, view).
        m.addAction(self.open_settings_action)
        self.recent_menu = m.addMenu("Open &recent settings")
        self.recent_menu.aboutToShow.connect(self._fill_recent_menu)
        m.addActions([self.save_settings_action, self.save_settings_as_action])
        m.addSeparator()
        m.addAction(self.reconnect_action)
        m.addSeparator()
        m.addAction(self.quit_action)

        m = self.menuBar().addMenu("&View")
        m.addActions([self.live_action, self.grid_action, self.crosshair_action])
        m.addSeparator()
        m.addActions([self.fit_action, self.actual_action, self.zoom_in_action, self.zoom_out_action])
        m.addSeparator()
        m.addAction(self.compare_action)
        m.addSeparator()
        # Filled in by _add_dock(), one entry per panel.
        self.panels_menu = m.addMenu("&Panels")

        m = self.menuBar().addMenu("&Measure")
        m.addActions([self.export_action, self.clear_measurements_action])
        m.addSeparator()
        m.addAction(self.export_counts_action)
        m.addSeparator()
        m.addActions([self.find_fibers_action, self.export_fibers_action])

        m = self.menuBar().addMenu("&Process")
        flat_menu = m.addMenu("&Flat-field correction")
        flat_menu.addActions([self.flat_info_action, self.flat_set_action, self.flat_load_action])
        flat_menu.addSeparator()
        flat_menu.addActions([self.flat_live_action, self.flat_apply_action])
        m.addSeparator()
        m.addActions(list(self.process_actions.values()))

        m = self.menuBar().addMenu("&Help")
        m.addAction(self._action("&About CamControl", None, self.show_about))

    def _build_measure_dock(self):
        panel = self.measure_panel
        for i, pane in enumerate(self.panes):
            pane.view.set_measurements(pane.measurements, panel.calibration)
            pane.view.activated.connect(lambda i=i: self.set_active(i))
            pane.view.measurement_drawn.connect(self._on_measurement_drawn)
            pane.view.tool_exit_requested.connect(lambda: panel.set_tool(None))
            panel.tool_changed.connect(pane.view.set_tool)  # tools work on either image

        def redraw():
            for i, pane in enumerate(self.panes):  # calibration may have changed for both
                selected = panel.selected_id() if i == self.active else None
                pane.view.set_measurements(pane.measurements, panel.calibration, selected)

        panel.selection_changed.connect(lambda sel: self._active_view().set_selected(sel))
        panel.changed.connect(redraw)
        panel.export_requested.connect(self.export_measurements)
        # Measuring and counting both use clicks, so only one is on at a time.
        panel.tool_changed.connect(lambda tool: tool and self.count_panel.set_counting(False))

        self.measure_dock = self._add_dock("Measurements", "measure_dock", panel,
                                           Qt.DockWidgetArea.BottomDockWidgetArea, "Ctrl+2")

    def _build_count_dock(self):
        panel = self.count_panel

        def on_mode(cls):
            if cls is not None:
                self.measure_panel.set_tool(None)
            for pane in self.panes:
                pane.view.set_count_class(cls)

        panel.mode_changed.connect(on_mode)
        panel.changed.connect(lambda: self._active_view().viewport().update())
        panel.export_requested.connect(self.export_counts)
        for pane in self.panes:
            view = pane.view
            view.set_counter(pane.counter)
            # The click made this view active first, so the panel holds its marks.
            view.count_add.connect(panel.add)
            view.count_remove.connect(panel.remove_near)
            view.count_undo.connect(panel.undo)
            view.tool_exit_requested.connect(lambda: panel.set_counting(False))

        self.count_dock = self._add_dock("Counting", "count_dock", panel,
                                         Qt.DockWidgetArea.BottomDockWidgetArea, "Ctrl+4")
        self.count_dock.hide()  # same edge as Measurements; its stripe button opens it

    def _build_fiber_dock(self):
        panel = self.fiber_panel
        panel.find_requested.connect(self.find_fiber_axes)
        panel.method_changed.connect(lambda: panel.axes and self.find_fiber_axes(quiet=True))
        panel.changed.connect(lambda: self._active_view().set_fiber_axes(panel.axes, panel.selected()))
        panel.export_requested.connect(self.export_fiber_axes)
        panel.live_changed.connect(lambda on: on and self.find_fiber_axes())
        self.fiber_dock = self._add_dock("Fiber axis", "fiber_dock", panel,
                                         Qt.DockWidgetArea.BottomDockWidgetArea, "Ctrl+5")
        self.fiber_dock.hide()  # same edge as Measurements; its stripe button opens it

    def _build_gallery_dock(self):
        self.gallery.open_requested.connect(self.open_image_path)
        self.gallery.folder_changed.connect(self._update_next_name)
        self.gallery.process_requested.connect(self.run_process)
        self.gallery.compare_requested.connect(self.compare_path)
        # On the left by default: each edge shows one panel at a time, and
        # Controls has the right.
        self.gallery_dock = self._add_dock("Captures", "gallery_dock", self.gallery,
                                           Qt.DockWidgetArea.LeftDockWidgetArea, "Ctrl+3")

    def _build_compare(self):
        self.compare.open_requested.connect(self.open_compare)
        self.compare.save_requested.connect(self.save_compare)
        self.compare.close_requested.connect(lambda: self._show_compare(False))
        self.compare.view.cursor_moved.connect(self._on_cursor)

    def _place_gallery_default(self):
        """Default spot for the Captures panel: the left edge."""
        self.addDockWidget(Qt.DockWidgetArea.LeftDockWidgetArea, self.gallery_dock)
        self.gallery_dock.show()

    def _add_dock(self, title, object_name, widget, area, shortcut, scroll=False):
        """Add a panel that can be moved, floated or minimized.

        Minimized panels come back from their button on the edge stripe,
        View > Panels, or the shortcut.
        Which panels are open is saved with the window layout.
        scroll: put the panel in a scroll area, so a tall panel scrolls
        instead of forcing the whole window to be taller than the screen.
        """
        dock = QDockWidget(title, self)
        dock.setObjectName(object_name)  # needed for saveState()
        if scroll:
            area_widget = QScrollArea()
            area_widget.setWidget(widget)
            area_widget.setWidgetResizable(True)
            area_widget.setFrameShape(QScrollArea.Shape.NoFrame)
            # Scroll up/down only: the panel can't be made narrower than its
            # contents (which would cut them off on the right).
            area_widget.setHorizontalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAlwaysOff)
            area_widget.setMinimumWidth(widget.minimumSizeHint().width()
                                        + area_widget.verticalScrollBar().sizeHint().width())
            widget = area_widget
        dock.setWidget(widget)
        # No floating: panels are moved by dragging their stripe button (or title).
        dock.setFeatures(QDockWidget.DockWidgetFeature.DockWidgetMovable
                         | QDockWidget.DockWidgetFeature.DockWidgetClosable)
        self.addDockWidget(area, dock)
        # Qt's ready-made show/hide action: checked while the panel is visible.
        toggle = dock.toggleViewAction()
        toggle.setShortcut(QKeySequence(shortcut))
        self.addAction(toggle)  # shortcut works even while the panel is hidden
        self.panels_menu.addAction(toggle)
        self.stripes.add(dock, area)  # its button on the edge stripe
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

        # Camera: connection status, then one slider per control. Ranges are
        # filled in when the camera opens and reports them (_on_camera_opened).
        cam_box = QGroupBox("Camera")
        cam_layout = QVBoxLayout(cam_box)
        status_row = QHBoxLayout()
        self.camera_status = QLabel("Connecting...")
        status_row.addWidget(self.camera_status, stretch=1)
        reconnect = QPushButton("Reconnect")
        reconnect.setToolTip("Connect to the camera again, e.g. after turning it on or plugging it in.")
        reconnect.clicked.connect(self.reconnect_camera)
        status_row.addWidget(reconnect)
        cam_layout.addLayout(status_row)
        self.slider_box = QWidget()
        form = QFormLayout(self.slider_box)
        form.setContentsMargins(0, 0, 0, 0)
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
        self.slider_box.setEnabled(False)  # until the camera opens
        cam_layout.addWidget(self.slider_box)
        layout.addWidget(cam_box)

        # Output: where photos, videos and time-lapses go (shared by all three).
        out_box = QGroupBox("Output")
        form = QFormLayout(out_box)
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

        flat_button = self._tool_button(self.flat_live_action)
        flat_button.setToolTip("Correct uneven lighting in the live view, photos, videos and time-lapses.\n"
                               "Set a flat reference first: Process > Flat-field correction.")
        form.addRow(flat_button)
        layout.addWidget(out_box)

        # Photo / Video / Time-lapse: one tab each, so only the options for
        # what you're doing take up room.
        photo_page = QWidget()
        form = QFormLayout(photo_page)
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
        # Keep the "Next:" preview current.
        self.name_edit.textChanged.connect(self._update_next_name)
        self.format_combo.currentIndexChanged.connect(self._update_next_name)

        capture_button = self._tool_button(self.capture_action)
        capture_button.setMinimumHeight(40)
        form.addRow(capture_button)
        self.last_saved_label = QLabel("")
        self.last_saved_label.setWordWrap(True)
        form.addRow(self.last_saved_label)

        rec = self.record_panel
        rec.record_requested.connect(self._request_recording)
        rec.timelapse_requested.connect(self._request_timelapse)
        rec.set_enabled(False)  # until the camera opens
        self.capture_tabs = QTabWidget()
        self.capture_tabs.addTab(photo_page, "Photo")
        self.capture_tabs.addTab(rec.video_page, "Video")
        self.capture_tabs.addTab(rec.timelapse_page, "Time-lapse")
        layout.addWidget(self.capture_tabs)

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
                                            Qt.DockWidgetArea.RightDockWidgetArea, "Ctrl+1", scroll=True)

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
        self.capture_tabs.setCurrentIndex(int(s.value("capture/tab", 0)))
        self.name_edit.setText(s.value("capture/name", ""))
        self.gallery.set_count(int(s.value("gallery/count", 12)))
        self.fiber_panel.set_method(s.value("fiber/method", "symmetry"))
        self.fiber_panel.parts_centre_check.setChecked(s.value("fiber/parts_centre", False, type=bool))
        try:
            self.record_panel.set_state(json.loads(s.value("recording/state", "{}")))
        except (ValueError, TypeError):
            pass  # unreadable saved state: keep the defaults
        self.gallery.set_folder(self.folder_edit.text())
        self._update_next_name()
        flat_path = s.value("flatfield/path", "")
        if flat_path and Path(flat_path).is_file():
            self._use_flat(flat_path, quiet=True)
            if s.value("flatfield/on", False, type=bool):
                self._set_flat_live(True)
        # Set the actions, then the view (setChecked doesn't fire triggered).
        for action, key, setter in (
            (self.grid_action, "view/grid", self.view.set_show_grid),
            (self.crosshair_action, "view/crosshair", self.view.set_show_crosshair),
        ):
            on = s.value(key, False, type=bool)
            action.setChecked(on)
            setter(on)
        # A layout saved while Recording was a tab under Captures (2026-09-28)
        # could be taller than the screen, so it isn't restored; the default
        # layout is used once instead.
        if s.contains("window/geometry") and not (s.value("window/has_recording", False, type=bool)
                                                  and not s.value("window/has_recording_dock", False, type=bool)):
            self.restoreGeometry(s.value("window/geometry"))
            self.restoreState(s.value("window/state"))
            self._set_corners()  # the saved layout includes the old corner settings
            # A layout saved before the Captures panel existed doesn't know
            # where it goes.
            if not s.value("window/has_gallery", False, type=bool):
                self._place_gallery_default()
            self._untabify()  # layouts saved before 2026-09-29 had tab groups
        else:
            self.resize(1400, 850)
            QTimer.singleShot(0, lambda: self.resizeDocks(
                [self.gallery_dock, self.controls_dock], [300, 380], Qt.Orientation.Horizontal))
        # Stripe button order, and at most one open panel per edge.
        self.stripes.set_order(s.value("panels/order", ""))
        # The settings file used last, if it's still there.
        recent = self._recent_settings()
        if recent:
            self.open_settings_path(recent[0], quiet=True)

    def _untabify(self):
        """Take panels out of Qt tab groups (see the dock options in __init__)."""
        for dock in (self.controls_dock, self.measure_dock, self.count_dock, self.gallery_dock):
            for other in self.tabifiedDockWidgets(dock):
                area = self.dockWidgetArea(other)
                hidden = other.isHidden()
                self.removeDockWidget(other)
                self.addDockWidget(area, other)
                other.setHidden(hidden)

    def _set_corners(self):
        """Side panels own the corners, so they always run the full height of
        the window: a bottom panel sits between the left and right panels,
        under the image, instead of spreading under them."""
        for corner, area in (
            (Qt.Corner.TopLeftCorner, Qt.DockWidgetArea.LeftDockWidgetArea),
            (Qt.Corner.BottomLeftCorner, Qt.DockWidgetArea.LeftDockWidgetArea),
            (Qt.Corner.TopRightCorner, Qt.DockWidgetArea.RightDockWidgetArea),
            (Qt.Corner.BottomRightCorner, Qt.DockWidgetArea.RightDockWidgetArea),
        ):
            self.setCorner(corner, area)

    def _save_settings(self):
        s = self.settings
        s.setValue("capture/folder", self.folder_edit.text())
        s.setValue("capture/average_idx", self.average_combo.currentIndex())
        s.setValue("capture/size_idx", self.size_combo.currentIndex())
        s.setValue("capture/format_idx", self.format_combo.currentIndex())
        s.setValue("capture/tab", self.capture_tabs.currentIndex())
        s.setValue("capture/name", self.name_edit.text())
        s.setValue("gallery/count", self.gallery.count)
        s.setValue("fiber/method", self.fiber_panel.method())
        s.setValue("fiber/parts_centre", self.fiber_panel.parts_centre_check.isChecked())
        s.setValue("view/grid", self.grid_action.isChecked())
        s.setValue("view/crosshair", self.crosshair_action.isChecked())
        s.setValue("window/geometry", self.saveGeometry())
        s.setValue("window/state", self.saveState())
        s.setValue("window/has_gallery", True)
        s.setValue("window/has_counting", True)
        s.setValue("panels/order", self.stripes.order())
        s.setValue("window/has_recording", True)
        s.setValue("window/has_recording_dock", True)
        s.setValue("recording/state", json.dumps(self.record_panel.state()))
        s.setValue("flatfield/path", self._flat_path or "")
        s.setValue("flatfield/on", self.flat_live_action.isChecked())

    # --- camera connection --------------------------------------------------------

    def _start_worker(self):
        """Open the camera on a new background thread."""
        self._set_camera_status("Connecting...")
        w = CameraWorker(self.camera_index)
        w.opened.connect(self._on_camera_opened)
        w.open_failed.connect(self._on_open_failed)
        w.frame_ready.connect(self._on_frame)
        w.settings_changed.connect(self._on_camera_settings)
        w.capture_started.connect(self._on_capture_started)
        w.capture_done.connect(self._on_capture_done)
        w.error.connect(self._on_error)
        w.recording_changed.connect(self._on_recording_changed)
        w.recording_progress.connect(self.record_panel.set_record_progress)
        w.timelapse_progress.connect(self._on_timelapse_progress)
        w.timelapse_finished.connect(self._on_timelapse_finished)
        if self.flat_live_action.isChecked():  # turned on from saved settings or a settings file
            w.set_flat_field(self.flat)
        self.worker = w
        w.start()

    def _set_camera_status(self, text: str, style: str = "", tooltip: str = ""):
        self.camera_status.setText(text)
        self.camera_status.setStyleSheet(style)
        self.camera_status.setToolTip(tooltip)

    def _on_open_failed(self, message: str):
        self.camera_open = False
        self._set_camera_status("Not connected", "color: #c00; font-weight: bold;", message)
        self.statusBar().showMessage(f"{message} Turn the camera on, then click Reconnect.")

    def reconnect_camera(self):
        """Close the camera (if open) and open it again."""
        busy = self._busy_recording()
        if busy and QMessageBox.question(
            self, "Reconnect camera", f"{' and '.join(busy).capitalize()}. Stop it and reconnect?"
        ) != QMessageBox.StandardButton.Yes:
            return
        self.statusBar().showMessage("Reconnecting to the camera...")
        self.worker.stop()  # closes the camera; a recording is finished and saved
        self.camera_open = False
        self.slider_box.setEnabled(False)
        self.capture_action.setEnabled(False)
        self.record_panel.set_enabled(False)
        self._start_worker()

    def _busy_recording(self) -> list[str]:
        return [what for what, on in (("a video is recording", self.record_panel.record_button.isChecked()),
                                      ("a time-lapse is running", self.record_panel.tl_button.isChecked())) if on]

    # --- camera events ------------------------------------------------------------

    def _on_camera_opened(self, ranges: dict):
        for name, slider in self.sliders.items():
            r = ranges.get(name)
            slider.setEnabled(r is not None)
            if r is None:
                continue
            slider.blockSignals(True)
            slider.setRange(r["min"], r["max"])
            slider.blockSignals(False)
        self.camera_open = True
        self._set_camera_status("Connected", "color: green;")
        self.slider_box.setEnabled(True)
        self.capture_action.setEnabled(True)
        self.record_panel.set_enabled(True)
        self.statusBar().showMessage("Camera open", 3000)
        if self._pending_camera:
            self._apply_camera_values(self._pending_camera)
            self._pending_camera = None

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
        # Fiber axes on the live view, a few times a second.
        if self.fiber_panel.live_check.isChecked() and now - self._fiber_last >= FIBER_LIVE_INTERVAL_S:
            self._fiber_last = now
            self.find_fiber_axes(quiet=True, pane_index=0)  # the live image is the left one

    def _on_camera_settings(self, values: dict):
        self._camera_values.update(values)
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
        self.capture_action.setEnabled(self.camera_open)
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
            self._result = None
            self._close_video()
        else:
            self.fps_label.clear()
            self._frame_times.clear()
        self._set_title()
        self._show_source()

    def _show_source(self, path: str | None = None):
        if path:
            self.source_label.setText(Path(path).name)
            self.source_label.setToolTip(str(path))
        elif self._result is not None:
            self.source_label.setText(f"{self._source} (unsaved)")
            self.source_label.setToolTip("Processing result. File > Save image as (Ctrl+S) to keep it.")
        else:
            self.source_label.setText("Live" if self.live else "Frozen")
            self.source_label.setToolTip("")
        self.compare.set_main_title(self.source_label.text(), self.source_label.toolTip())

    def _set_title(self, file_name: str | None = None):
        title = f"CamControl - {file_name or ('live' if self.live else 'frozen')}"
        if self.settings_path:
            title += f"   [{Path(self.settings_path).stem}]"
        self.setWindowTitle(title)

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
        folder = QFileDialog.getExistingDirectory(self, "Output folder", self.folder_edit.text())
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
            self, "Open image or video", self.folder_edit.text(),
            "Images and videos (*.tif *.tiff *.png *.jpg *.jpeg *.bmp *.mp4 *.avi);;All files (*)",
        )
        if path:
            self.open_image_path(path)

    def open_image_path(self, path: str):
        """Show an image file in the view (freezes the live feed). Videos play."""
        if is_video(path):
            self.open_video_path(path)
            return
        img = load_image_file(path)
        if img is None:
            QMessageBox.warning(self, "CamControl", f"Could not read {path}")
            return
        self._ask_clear_annotations("Open image")
        self._set_live(False)  # stop the camera feed replacing it
        self._close_video()
        self.view.set_image(img)
        h, w = img.shape[:2]
        self.size_label.setText(f"{w} x {h}")
        self._source = Path(path).name
        self._result = None
        self._set_title(self._source)
        self._show_source(path)

    def open_video_path(self, path: str):
        """Open a video under the player bar, paused on its first frame."""
        self._ask_clear_annotations("Open video")
        self._set_live(False)
        if not self.player.open(path):  # shows the first frame via _on_video_frame
            self._close_video()
            QMessageBox.warning(self, "CamControl", f"Could not play {path}")
            return
        self.player.show()
        self._source = Path(path).name
        self._result = None
        self._set_title(self._source)
        self._show_source(path)
        self.player.play()

    def _on_video_frame(self, frame):
        self.view.set_image(frame)
        h, w = frame.shape[:2]
        self.size_label.setText(f"{w} x {h}")

    def _close_video(self):
        self.player.close_video()
        self.player.hide()

    # --- which image the panels work on --------------------------------------------

    def _active_view(self) -> ImageView:
        return self.panes[self.active].view

    def _active_source(self) -> str:
        """Name of the active image, for exports."""
        if self.active == 1 and self.compare.path:
            return Path(self.compare.path).name
        return self._source

    def set_active(self, index: int):
        """Make the panels show (and work on) the left (0) or right (1) image."""
        if index == self.active or (index == 1 and not self.compare.comparing):
            return
        old = self.panes[self.active]
        old.next_id = self.measure_panel.next_id
        old.fiber_axes = self.fiber_panel.axes
        old.view.set_selected(None)
        old.view.set_fiber_axes(old.fiber_axes)
        self.active = index
        new = self.panes[index]
        self.measure_panel.set_state(new.measurements, new.next_id)
        self.count_panel.set_counter(new.counter)
        self.fiber_panel.set_axes(new.fiber_axes)
        self.compare.set_active(index)
        side = "right" if index else "left"
        self.statusBar().showMessage(f"Working on the {side} image: {self._active_source()}", 4000)

    def _clear_pane(self, index: int):
        pane = self.panes[index]
        pane.measurements.clear()
        pane.next_id = 1
        pane.counter.marks.clear()
        pane.fiber_axes = []
        pane.view.set_fiber_axes([])
        if index == self.active:
            self.measure_panel.set_state(pane.measurements, 1)
            self.count_panel.set_counter(pane.counter)
            self.fiber_panel.set_axes([])
        pane.view.viewport().update()

    def _ask_clear_annotations(self, title: str):
        """Measurements and counts belong to the image they were made on."""
        self.set_active(0)  # the left image is the one changing
        if not (self.measure_panel.measurements or self.count_panel.counter.marks):
            return
        if QMessageBox.question(
            self, title, "Clear the current measurements and counts? They were made on a different image."
        ) == QMessageBox.StandardButton.Yes:
            self.measure_panel.clear(confirm=False)
            self.count_panel.clear(confirm=False)

    def save_image(self):
        """Save the active image: a processing result, a frozen frame or a copy of a file."""
        image = self._active_view().image
        if image is None:
            return
        on_right = self.active == 1
        stem = self._result["short_name"] if self._result and not on_right else "image"
        default = next_capture_path(Path(self.folder_edit.text()), stem, "tif")
        path, _ = QFileDialog.getSaveFileName(
            self, "Save image", str(default), "TIFF (*.tif);;PNG (*.png);;JPEG (*.jpg)")
        if not path:
            return
        path = Path(path)
        if path.suffix.lower() not in (".tif", ".tiff", ".png", ".jpg", ".jpeg"):
            path = path.with_suffix(".tif")
        meta = {"timestamp": datetime.now().isoformat(timespec="seconds"),
                "saved_size": [image.shape[1], image.shape[0]]}
        if on_right:
            meta["source"] = self._active_source()
        elif self._result:
            meta.update(self._result["meta"])
        else:
            meta["source"] = self._source
            if self.player.is_open:
                self.player.pause()
                meta["video_frame"] = self.player.position + 1
        try:
            path.parent.mkdir(parents=True, exist_ok=True)
            save_image_file(path, image)
            path.with_suffix(".json").write_text(json.dumps(meta, indent=2))
        except Exception as e:
            QMessageBox.warning(self, "Save image", f"Save failed: {e}")
            return
        self.statusBar().showMessage(f"Saved {path}", 5000)
        if self._result and not on_right:  # it's a file now
            self._result = None
            self._source = path.name
            self._set_title(self._source)
            self._show_source(str(path))
        self.gallery.refresh(select=str(path))

    # --- processing -----------------------------------------------------------------

    def show_result(self, image, title: str, short_name: str, meta: dict):
        """Show a processing result in the view, frozen and unsaved."""
        self._ask_clear_annotations(title)
        self._set_live(False)
        self._close_video()
        self.view.set_image(image)
        h, w = image.shape[:2]
        self.size_label.setText(f"{w} x {h}")
        self._source = title
        self._result = {"short_name": short_name, "meta": meta}
        self._set_title(f"{title} (unsaved)")
        self._show_source()
        self.statusBar().showMessage(f"{title} done. File > Save image as (Ctrl+S) to keep it.", 8000)

    def run_process(self, key: str, paths: list | None = None):
        """Open a processing dialog, then run it in the background.

        paths: images to start with; by default, those selected in Captures.
        """
        if self._job is not None:
            return  # one at a time
        if key not in self._dialogs:
            self._dialogs[key] = PROCESS_DIALOGS[key](self)
        dialog = self._dialogs[key]
        dialog.start_folder = self.folder_edit.text()
        if paths is None:
            selected = [p for p in self.gallery.selected_paths() if not is_video(p)]
            paths = selected if len(selected) >= dialog.min_images else None
        if paths:
            dialog.set_paths(paths)
        if not dialog.exec():  # cancelled
            return

        paths = dialog.paths()
        settings = dialog.settings()
        process = dialog.process

        def work():  # runs on the background thread
            images = []
            for p in paths:
                img = load_image_file(p)
                if img is None:
                    raise ValueError(f"Could not read {p}")
                images.append(img)
            return process(images, settings)

        meta = {"processing": dialog.title, "inputs": [Path(p).name for p in paths],
                "input_folder": str(Path(paths[0]).parent), "settings": settings}
        title = f"{dialog.title} of {len(paths)} image{'s' if len(paths) > 1 else ''}"

        def done(image):
            self._job = None
            self.show_result(image, title, dialog.short_name, meta)

        def failed(message):
            self._job = None
            QMessageBox.warning(self, dialog.title, message)

        self._job = run_job(self, f"{dialog.title}: working on {len(paths)} images...", work, done, failed)

    # --- flat-field -----------------------------------------------------------------

    def _use_flat(self, path: str, quiet: bool = False) -> bool:
        img = load_image_file(path)
        if img is None:
            if not quiet:
                QMessageBox.warning(self, "Flat-field", f"Could not read {path}")
            return False
        self.flat = FlatField(img, Path(path).name)
        self._flat_path = str(path)
        if self.flat_live_action.isChecked():
            self.worker.set_flat_field(self.flat)  # swap in the new reference
        self._update_flat_actions()
        return True

    def _update_flat_actions(self):
        has = self.flat is not None
        self.flat_info_action.setText(
            f"Reference: {self.flat.name}" if has else "No flat reference (use or load one below)")
        self.flat_live_action.setEnabled(has)
        self.flat_apply_action.setEnabled(has)
        if not has:
            self.flat_live_action.setChecked(False)

    def set_flat_from_view(self):
        """Save the image in the view as the flat reference and use it."""
        image = self.view.image
        if image is None:
            return
        if self.live and self.flat_live_action.isChecked():
            QMessageBox.information(
                self, "Flat-field",
                "The live view is already corrected. Turn off \"Correct live view and captures\" first, "
                "so the reference is taken from the uncorrected image.")
            return
        if QMessageBox.question(
            self, "Flat-field",
            "Use the image in the view as the flat reference?\n\n"
            "It should show an empty, evenly lit field (e.g. a blank slide) with the same "
            "lighting and zoom you'll use for samples. Averaging several frames when "
            "capturing it gives a cleaner reference."
        ) != QMessageBox.StandardButton.Yes:
            return
        FLAT_DIR.mkdir(parents=True, exist_ok=True)
        path = FLAT_DIR / f"flat_{time.strftime('%Y%m%d_%H%M%S')}.tif"
        save_image_file(path, image)
        if self._use_flat(str(path)):
            self.statusBar().showMessage(f"Flat reference saved to {path}", 8000)

    def load_flat(self):
        start = str(FLAT_DIR) if FLAT_DIR.is_dir() else self.folder_edit.text()
        path, _ = QFileDialog.getOpenFileName(self, "Load flat reference", start, FILE_FILTER)
        if path and self._use_flat(path):
            self.statusBar().showMessage(f"Flat reference: {Path(path).name}", 5000)

    def _set_flat_live(self, on: bool):
        on = on and self.flat is not None
        self.flat_live_action.setChecked(on)
        if hasattr(self, "worker"):  # at startup, __init__ sends it once the worker exists
            self.worker.set_flat_field(self.flat if on else None)

    def apply_flat_to_view(self):
        image = self.view.image
        if image is None or self.flat is None:
            return
        if self.live:
            QMessageBox.information(self, "Flat-field",
                                    "This corrects a frozen or opened image. For the live view, "
                                    "turn on \"Correct live view and captures\".")
            return
        name = self._source
        self.show_result(self.flat.apply(image), f"{name} (flat-field corrected)", "flat",
                         {"processing": "Flat-field correction", "inputs": [name],
                          "flat_field": self.flat.name})

    # --- measurements ---------------------------------------------------------------

    def _on_measurement_drawn(self, kind: str, points: list):
        error = self.measure_panel.add(kind, points)
        if error:
            self.statusBar().showMessage(error, 5000)

    def export_measurements(self):
        """Save the table (Excel or CSV) plus the image and an annotated copy."""
        panel = self.measure_panel
        if not panel.measurements or self._active_view().image is None:
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
        image = self._active_view().image
        source = self._active_source()
        try:
            export_table(measurement_rows(panel.measurements, cal, source), path, cal, source)
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

    def export_counts(self):
        """Save the counts (Excel or CSV) plus the image and a copy with the marks."""
        counter = self.count_panel.counter
        if not counter.marks or self._active_view().image is None:
            QMessageBox.information(self, "Export counts", "There are no counts to export.")
            return
        default = Path(self.folder_edit.text()) / f"counts_{time.strftime('%Y%m%d_%H%M%S')}.xlsx"
        path, _ = QFileDialog.getSaveFileName(
            self, "Export counts", str(default), "Excel (*.xlsx);;CSV (*.csv)")
        if not path:
            return
        path = Path(path)
        if path.suffix.lower() not in (".xlsx", ".csv"):
            path = path.with_suffix(".xlsx")
        path.parent.mkdir(parents=True, exist_ok=True)
        image = self._active_view().image
        try:
            counter.export(path, self._active_source())
            image_path = path.with_name(f"{path.stem}_image.png")
            marked_path = path.with_name(f"{path.stem}_marked.png")
            to_qimage(image).save(str(image_path))
            render_marks(image, counter).save(str(marked_path))
        except Exception as e:  # e.g. the file is open in Excel
            QMessageBox.warning(self, "Export counts", f"Export failed: {e}")
            return
        self.statusBar().showMessage(
            f"Exported {len(counter.marks)} marks to {path.name}, {image_path.name} and {marked_path.name}", 8000)

    def find_fiber_axes(self, quiet: bool = False, pane_index: int | None = None):
        """Measure the fiber axes in the active image (or the given pane's)."""
        index = self.active if pane_index is None else pane_index
        pane = self.panes[index]
        image = pane.view.image
        if image is None:
            return
        circles = None
        if self.fiber_panel.use_circles.isChecked():
            # Fibers marked with Circle measurements (in pixels, whatever the calibration).
            circles = []
            for m in pane.measurements:
                if m.kind in CIRCLE_KINDS:
                    r = compute(m)
                    circles.append((*r["center_px"], r["radius"]))
        axes = analyse_fibers(image, circles or None, method=self.fiber_panel.method(),
                              parts_centre=self.fiber_panel.parts_centre_check.isChecked())
        if index == self.active:
            self.fiber_panel.set_axes(axes)
        else:  # e.g. live updates of the left image while the right one is active
            pane.fiber_axes = axes
            pane.view.set_fiber_axes(axes)
        if not quiet:
            if axes:
                unclear = sum(not f.clear for f in axes)
                note = f" ({unclear} unclear)" if unclear else ""
                self.statusBar().showMessage(f"Found {len(axes)} fiber{'s' if len(axes) != 1 else ''}{note}.", 6000)
            else:
                self.statusBar().showMessage(
                    "No fibers found. Draw a Circle (3 pt or centre) round the fiber in Measurements, then Find axes.", 10000)
            if self.fiber_dock.isHidden():  # show the results (minimizing the others on its edge)
                self.fiber_dock.toggleViewAction().trigger()

    def export_fiber_axes(self):
        """Save the fiber table (Excel or CSV) plus the image and a copy with the axes drawn."""
        panel = self.fiber_panel
        if not panel.axes or self._active_view().image is None:
            QMessageBox.information(self, "Export fiber axes", "There are no fiber axes to export. Click Find axes first.")
            return
        default = Path(self.folder_edit.text()) / f"fiber_axes_{time.strftime('%Y%m%d_%H%M%S')}.xlsx"
        path, _ = QFileDialog.getSaveFileName(
            self, "Export fiber axes", str(default), "Excel (*.xlsx);;CSV (*.csv)")
        if not path:
            return
        path = Path(path)
        if path.suffix.lower() not in (".xlsx", ".csv"):
            path = path.with_suffix(".xlsx")
        path.parent.mkdir(parents=True, exist_ok=True)
        image = self._active_view().image
        try:
            table = pd.DataFrame(panel.rows(self._active_source()))
            if path.suffix.lower() == ".xlsx":
                table.to_excel(path, index=False, sheet_name="Fiber axes")
            else:
                table.to_csv(path, index=False, encoding="utf-8-sig")
            image_path = path.with_name(f"{path.stem}_image.png")
            drawn_path = path.with_name(f"{path.stem}_axes.png")
            to_qimage(image).save(str(image_path))
            render_fiber_axes(image, panel.axes).save(str(drawn_path))
        except Exception as e:  # e.g. the file is open in Excel
            QMessageBox.warning(self, "Export fiber axes", f"Export failed: {e}")
            return
        self.statusBar().showMessage(
            f"Exported {len(panel.axes)} fibers to {path.name}, {image_path.name} and {drawn_path.name}", 8000)

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

    # --- settings files ------------------------------------------------------------------

    def current_settings(self) -> dict:
        data = {
            "capture": {
                "average": self.average_combo.currentData(),
                "save_size": list(self.size_combo.currentData()),
                "format": self.format_combo.currentData(),
            },
            "output": {"folder": self.folder_edit.text(), "name": self.name_edit.text()},
            "flat_field": {"path": self._flat_path, "on": self.flat_live_action.isChecked()},
            "recording": self.record_panel.state(),
            "view": {"grid": self.grid_action.isChecked(), "crosshair": self.crosshair_action.isChecked()},
        }
        # What the camera last reported, or what's waiting to be applied.
        camera = self._camera_values or self._pending_camera
        if camera:
            data["camera"] = {n: v for n, v in camera.items() if n in self.sliders}
        return data

    def _recent_settings(self) -> list[str]:
        recent = self.settings.value("settings_files/recent", []) or []
        if isinstance(recent, str):  # QSettings returns a lone string for a one-item list
            recent = [recent]
        return [p for p in recent if Path(p).is_file()]

    def _remember_settings_file(self, path):
        self.settings_path = str(path)
        self.settings.setValue("settings_files/recent",
                               settings_file.add_recent(self._recent_settings(), path))
        self._set_title(None if self._source == "live" else self._source)

    def _fill_recent_menu(self):
        menu = self.recent_menu
        menu.clear()
        recent = self._recent_settings()
        for path in recent:
            menu.addAction(Path(path).name, lambda p=path: self.open_settings_path(p)).setToolTip(path)
        if not recent:
            menu.addAction("(none yet)").setEnabled(False)
        else:
            menu.addSeparator()
            menu.addAction("Clear list", lambda: self.settings.setValue("settings_files/recent", []))
        menu.setToolTipsVisible(True)

    def _settings_start_folder(self) -> str:
        if self.settings_path:
            return str(Path(self.settings_path).parent)
        return str(settings_file.SETTINGS_DIR)

    def open_settings(self):
        path, _ = QFileDialog.getOpenFileName(self, "Open settings", self._settings_start_folder(),
                                              settings_file.FILE_FILTER)
        if path:
            self.open_settings_path(path)

    def open_settings_path(self, path: str, quiet: bool = False):
        try:
            data = settings_file.load_settings_file(path)
        except (OSError, ValueError) as e:
            if not quiet:
                QMessageBox.warning(self, "Open settings", f"Could not open {Path(path).name}: {e}")
            return
        problems = self.apply_settings(data)
        self._remember_settings_file(path)
        message = f"Settings: {Path(path).name}"
        if problems:
            message += ". " + " ".join(problems)
        self.statusBar().showMessage(message, 8000)

    def save_settings(self):
        if not self.settings_path:
            self.save_settings_as()
            return
        self._write_settings(self.settings_path)

    def save_settings_as(self):
        start = self.settings_path or str(settings_file.SETTINGS_DIR / "settings.json")
        path, _ = QFileDialog.getSaveFileName(self, "Save settings as", start, settings_file.FILE_FILTER)
        if not path:
            return
        if Path(path).suffix.lower() != ".json":
            path += ".json"
        self._write_settings(path)

    def _write_settings(self, path: str):
        try:
            settings_file.save_settings_file(path, self.current_settings())
        except OSError as e:
            QMessageBox.warning(self, "Save settings", f"Could not save: {e}")
            return
        self._remember_settings_file(path)
        self.statusBar().showMessage(f"Saved settings to {path}", 6000)

    def apply_settings(self, data: dict) -> list[str]:
        """Apply a settings file's contents. Returns notes about anything that couldn't be applied."""
        problems = []
        camera = data.get("camera")
        if camera:
            if self.camera_open:
                self._apply_camera_values(camera)
            else:
                self._pending_camera = camera  # applied when the camera opens
                problems.append("Camera settings will be applied when the camera opens.")

        cap = data.get("capture", {})
        for combo, value in ((self.average_combo, cap.get("average")),
                             (self.size_combo, tuple(cap["save_size"]) if "save_size" in cap else None),
                             (self.format_combo, cap.get("format"))):
            i = combo.findData(value) if value is not None else -1
            if i >= 0:
                combo.setCurrentIndex(i)

        flat = data.get("flat_field", {})
        path = flat.get("path")
        if path and Path(path).is_file():
            if path != self._flat_path:
                self._use_flat(path)
            self._set_flat_live(bool(flat.get("on")))
        else:
            self._set_flat_live(False)
            if path:
                problems.append(f"Flat reference not found ({Path(path).name}); correction is off.")

        if "recording" in data:
            self.record_panel.set_state(data["recording"])

        out = data.get("output", {})
        if out.get("folder"):
            self.folder_edit.setText(out["folder"])
            self.gallery.set_folder(out["folder"])
        if "name" in out:
            self.name_edit.setText(out["name"])
        self._update_next_name()

        view = data.get("view", {})
        for action, key, setter in ((self.grid_action, "grid", self.view.set_show_grid),
                                    (self.crosshair_action, "crosshair", self.view.set_show_crosshair)):
            if key in view:
                action.setChecked(bool(view[key]))
                setter(bool(view[key]))
        return problems

    def _apply_camera_values(self, values: dict):
        for name, value in values.items():
            slider = self.sliders.get(name)
            if slider is not None and slider.isEnabled():
                slider.setValue(int(value))  # sends it to the camera via _on_slider

    # --- compare -------------------------------------------------------------------------

    def _show_compare(self, on: bool):
        if not on:
            self.set_active(0)
        self.compare_action.setChecked(on)
        self.compare.set_comparing(on)
        self.compare.set_active(self.active)

    def compare_path(self, path: str):
        if not self.compare.load(path):
            QMessageBox.warning(self, "Compare", f"Could not read {path}")
            return
        self._clear_pane(1)  # its measurements and marks were on the previous image
        self._show_compare(True)

    def open_compare(self):
        path, _ = QFileDialog.getOpenFileName(self, "Open image to compare", self.folder_edit.text(), FILE_FILTER)
        if path:
            self.compare_path(path)

    def save_compare(self):
        if self.view.image is None or self.compare.view.image is None:
            QMessageBox.information(self, "Compare", "Open an image to compare first.")
            return
        left_name = self._source if self._source != "live" else "live"
        right_name = Path(self.compare.path).name
        default = next_capture_path(Path(self.folder_edit.text()), "compare", "png")
        path, _ = QFileDialog.getSaveFileName(
            self, "Save side by side", str(default), "PNG (*.png);;TIFF (*.tif);;JPEG (*.jpg)")
        if not path:
            return
        path = Path(path)
        if path.suffix.lower() not in (".tif", ".tiff", ".png", ".jpg", ".jpeg"):
            path = path.with_suffix(".png")
        try:
            save_image_file(path, side_by_side(self.view.image, self.compare.view.image, left_name, right_name))
        except Exception as e:
            QMessageBox.warning(self, "Compare", f"Save failed: {e}")
            return
        self.statusBar().showMessage(f"Saved {path}", 5000)

    # --- video and time-lapse -----------------------------------------------------------

    def _request_recording(self, start: bool):
        if start:
            self.worker.start_recording(Path(self.folder_edit.text()), clean_name(self.name_edit.text()),
                                        **self.record_panel.video_settings())
        else:
            self.worker.stop_recording()

    def _on_recording_changed(self, on: bool, path: str):
        self.record_panel.set_recording(on, path)
        if on:
            self.statusBar().showMessage(f"Recording to {Path(path).name}...")
        elif path:
            self.statusBar().showMessage(f"Saved video {path}", 8000)
            self._update_next_name()

    def _request_timelapse(self, start: bool):
        if not start:
            self.worker.stop_timelapse()
            return
        opts = {
            "n_frames": self.average_combo.currentData(),
            "save_size": self.size_combo.currentData(),
            "fmt": self.format_combo.currentData(),
            "folder": Path(self.folder_edit.text()),
            "name": clean_name(self.name_edit.text()),
        }
        self.worker.start_timelapse(self.record_panel.make_timelapse(), opts)
        self.record_panel.set_timelapse_running(True)

    def _on_timelapse_progress(self, taken: int, total: int, next_due: float, path: str):
        self.record_panel.set_timelapse_progress(taken, total, next_due)
        self.last_saved_label.setText(f"Saved {Path(path).name} (time-lapse)")
        self._update_next_name()

    def _on_timelapse_finished(self, message: str):
        self.record_panel.set_timelapse_message(message)
        self.statusBar().showMessage(message, 10000)
        self._update_next_name()

    def closeEvent(self, event):
        busy = self._busy_recording()
        if busy and QMessageBox.question(
            self, "Quit", f"{' and '.join(busy).capitalize()}. Stop and quit?"
        ) != QMessageBox.StandardButton.Yes:
            event.ignore()
            return
        if self._job is not None:
            self._job.wait()  # let a running processing job finish first
        self._save_settings()
        self.worker.stop()
        super().closeEvent(event)
