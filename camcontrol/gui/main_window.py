"""Main window: the views in the middle (one or more, each showing a camera,
an image file, a video or a processing result), controls and captures
panels at the sides, measurements, counting and fiber axis panels at the
bottom.

Shortcuts:
    Space       capture
    L           live / freeze
    G           grid
    C           crosshair
    F           fit image to window
    1           100% (one image pixel per screen pixel)
    Ctrl+= / -  zoom in / out (or use the mouse wheel)
    Ctrl+O      open an image file (in the active view)
    Ctrl+S      save the image in the active view (e.g. a processing result)
    Ctrl+E      export measurements
    Ctrl+K      one view / several views
    Ctrl+Shift+K add a view
    Ctrl+1..5   show / hide the Controls / Measurements / Captures / Counting / Fiber axis panel

Views: each has its own source, live/frozen state, measurements, counting
marks and fiber axes. The panels work on the active view (the one clicked
last). Controls work on the camera of the active view, or, if it has none,
of the camera view used last. With Link views on, Capture, Live/Freeze,
Fit, 100% and Find fiber axes act on every view.

Processing tools (Process menu, or select images in Captures and right-click)
show their result in the active view, unsaved, until you press Ctrl+S.
"""

import json
import os
import re
import time
from dataclasses import asdict, dataclass, field
from datetime import datetime
from pathlib import Path

import pandas as pd
from PySide6.QtCore import QSettings, Qt, QThread, QTimer, Signal
from PySide6.QtGui import QAction, QActionGroup, QCursor, QKeySequence, QPixmap
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
    QMenu,
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

from camcontrol.camera import ADJUSTABLE, format_exposure, frame_sizes
from camcontrol.capture import CAPTURE_DIR, HD2_SCALE, clean_name, next_capture_path, scaled_size
from camcontrol.export import export_table, measurement_rows
from camcontrol import settings_file
from camcontrol.gui.camera_link import CameraLink
from camcontrol.gui.count_panel import CountPanel, render_marks
from camcontrol.gui.fiber_panel import FiberPanel, render_fiber_axes
from camcontrol.gui.overlay_settings import CrosshairStyle, GridStyle, OverlayDialog
from camcontrol.gui.gallery import GalleryPanel
from camcontrol.gui.image_view import ImageView
from camcontrol.gui.jobs import Job, run_job
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
from camcontrol.gui.views import LAYOUTS, ViewsArea, grid_image
from camcontrol.image_io import is_video, load_image_file, save_image_file
from camcontrol.paths import LOGO
from camcontrol.measure import CIRCLE_KINDS, compute
from camcontrol.processing.count import Counter
from camcontrol.processing.fiber_axis import Detection
from camcontrol.processing.fiber_axis import analyse as analyse_fibers
from camcontrol.processing.flatfield import FlatField
from camcontrol.uvc_controls import list_video_devices

AVERAGE_CHOICES = [1, 4, 8, 16, 32]
# Save size as a scale of the camera's frame size. Labels get the real sizes
# when the camera opens (_update_save_sizes).
SAVE_SCALES = [
    ("Native (camera size)", 1.0),
    ("HD2 size (1.7x upscaled)", HD2_SCALE),
]
FORMATS = [("TIFF", "tif"), ("PNG", "png")]
FLAT_DIR = CAPTURE_DIR.parent / "flats"  # flat-field references taken in the app
FIBER_LIVE_INTERVAL_S = 0.25  # how often "Update live" re-measures fiber axes (each live view)
# Process menu / Captures right-click tools: key -> dialog class.
PROCESS_DIALOGS = {
    "stack": FocusStackDialog,
    "hdr": HdrDialog,
    "stitch": StitchDialog,
    "composite": FluorescenceDialog,
}


@dataclass(eq=False)  # compared by identity: each is one view
class Pane:
    """One view of the window: what it shows and the measurements, counting
    marks and fiber axes made on it. The panels show the active one."""
    view: ImageView
    measurements: list
    counter: Counter
    next_id: int = 1
    fiber_axes: list = field(default_factory=list)
    camera: CameraLink | None = None  # its camera (kept open while it shows a file, so L returns to it)
    live: bool = False                # showing its camera's frames
    path: str | None = None           # the file shown
    title: str = ""                   # what it shows when not live: a file name, a result's title
    result: dict | None = None        # an unsaved processing result: short_name, meta
    fiber_last: float = 0.0           # when its live frames were last measured (Update live)


def camera_list() -> list[tuple[str, int]]:
    """The cameras Windows lists, as (name, n): n tells apart cameras with the
    same name (0 for the first). Position in the list = OpenCV camera index."""
    devices = []
    for name, _ in list_video_devices():
        devices.append((name, sum(1 for d in devices if d[0] == name)))
    return devices


def camera_label(key: tuple[str, int]) -> str:
    name, n = key
    return name if n == 0 else f"{name} ({n + 1})"


def find_item(combo: QComboBox, value) -> int:
    """Index of the item whose data equals value, or -1. (QComboBox.findData
    compares Python tuples by identity, not by value.)"""
    return next((i for i in range(combo.count()) if combo.itemData(i) == value), -1)


class _DeviceCombo(QComboBox):
    """A combo box that says when its list is about to open (to refresh it)."""
    about_to_show = Signal()

    def showPopup(self):
        self.about_to_show.emit()
        super().showPopup()


NO_CAMERA = ("", -1)  # Device box / source menu item: no camera in this view


class MainWindow(QMainWindow):
    def __init__(self, camera_index: int | None = None):
        """camera_index: open this camera in the first view; None = the one used last (else the first)."""
        super().__init__()
        self.settings = QSettings("CamControl", "CamControl")
        self.flat: FlatField | None = None
        self._flat_path: str | None = None
        self._dialogs = {}  # processing dialogs, kept so they remember their files and options
        self._job = None    # background processing job while one runs
        self.settings_path: str | None = None      # settings file opened or saved last
        self._cli_index = camera_index  # used once, when the camera list is first read
        self._camera_values: dict = {}  # settings-file values per camera label, applied when each opens

        self._set_corners()
        # No Qt tab groups: each edge shows one panel at a time and the stripe
        # buttons act as its tabs. (Qt's tab bars also leave stray tabs drawn
        # in the corner when a tabbed panel is hidden.)
        self.setDockOptions(QMainWindow.DockOption.AnimatedDocks | QMainWindow.DockOption.AllowNestedDocks)
        # PyCharm-style stripes along the edges: a button per panel to minimize / restore it.
        self.stripes = ToolStripes(self)

        # The views, and under them the video player bar (shown while a video is open).
        self.views = ViewsArea()
        self.player = VideoPlayer()
        self.player.hide()
        self.player.frame_ready.connect(self._on_video_frame)
        self.player.close_requested.connect(self._close_video_and_resume)
        self._video_pane: Pane | None = None  # the view the video plays in
        centre = QWidget()
        centre_layout = QVBoxLayout(centre)
        centre_layout.setContentsMargins(0, 0, 0, 0)
        centre_layout.setSpacing(0)
        centre_layout.addWidget(self.views, stretch=1)
        centre_layout.addWidget(self.player)
        self.setCentralWidget(centre)
        self.measure_panel = MeasurePanel()
        self.count_panel = CountPanel()
        self.fiber_panel = FiberPanel()
        self._capturing: set = set()        # cameras taking a photo now (Capture waits for them)
        self._fiber_job: Job | None = None  # fiber measuring running in the background (one at a time)
        self._fiber_waiting: dict = {}      # pane -> request made while it was busy (the newest per view)
        self._fiber_generation = 0  # counts settings changes: results measured with older settings are dropped
        self.grid_style, self.crosshair_style = GridStyle(), CrosshairStyle()  # on every view (_apply_overlays)
        self._overlay_dialog: OverlayDialog | None = None  # Grid and crosshair window, made when first opened
        self.panes: list[Pane] = []
        self.active: Pane | None = None      # the view the panels work on
        self.controlled: Pane | None = None  # the view whose camera Controls work on
        self.gallery = GalleryPanel()
        self.record_panel = RecordPanel(self)  # the Video and Time-lapse tabs in Controls

        self._build_actions()
        self._build_menus()
        self._build_controls()
        self._build_measure_dock()
        self._build_count_dock()
        self._build_fiber_dock()
        self._build_gallery_dock()
        self._build_status_bar()
        self._build_views()
        self._load_settings()

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
        self.capture_action.setEnabled(False)  # until a camera opens
        self.save_action = self._action("&Save image as...", "Ctrl+S", self.save_image)
        self.open_folder_action = self._action("Open captures &folder", None, self.open_capture_folder)
        self.reconnect_action = self._action("&Reconnect camera", None, self.reconnect_camera)
        self.open_settings_action = self._action("Open se&ttings...", None, self.open_settings)
        self.save_settings_action = self._action("Save setti&ngs", "Ctrl+Shift+S", self.save_settings)
        self.save_settings_as_action = self._action("Save settings &as...", None, self.save_settings_as)
        self.quit_action = self._action("&Quit", "Ctrl+Q", self.close)

        self.live_action = self._action("&Live", "L", self._toggle_live, checkable=True)
        self.grid_action = self._action("&Grid", "G", self._apply_overlays, checkable=True)
        self.grid_settings_action = self._action("Grid &settings...", None, lambda: self.show_overlay_settings("grid"))
        self.crosshair_action = self._action("C&rosshair", "C", self._apply_overlays, checkable=True)
        self.crosshair_settings_action = self._action("Crosshair s&ettings...", None,
                                                      lambda: self.show_overlay_settings("crosshair"))
        self.fit_action = self._action("&Fit to window", "F", lambda: [p.view.fit() for p in self._targets()])
        self.actual_action = self._action("&100%", "1", lambda: [p.view.actual_size() for p in self._targets()])
        self.zoom_in_action = self._action("Zoom &in", "Ctrl+=", lambda: self.active.view.zoom_in())
        self.zoom_out_action = self._action("Zoom &out", "Ctrl+-", lambda: self.active.view.zoom_out())

        # Views
        self.multi_action = self._action("&Several views", "Ctrl+K", self._toggle_multi, checkable=True)
        self.add_view_action = self._action("&Add view", "Ctrl+Shift+K", self._add_view_and_choose)
        self.open_views_action = self._action("Open images in new &views...", None, self.open_in_new_views)
        self.remove_view_action = self._action("&Remove this view", None, lambda: self.remove_view(self.active))
        self.link_action = self._action("&Link views", None, lambda on: self.views.set_linked(on), checkable=True)
        self.layout_actions = QActionGroup(self)
        for name, label in LAYOUTS.items():
            a = self._action(label, None, lambda _=False, n=name: self._set_layout(n), checkable=True)
            a.setData(name)
            self.layout_actions.addAction(a)

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
        m.addActions([self.open_action, self.open_views_action, self.save_action,
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
        m.addActions([self.live_action, self.grid_action, self.grid_settings_action, self.crosshair_action,
                      self.crosshair_settings_action])
        m.addSeparator()
        m.addActions([self.fit_action, self.actual_action, self.zoom_in_action, self.zoom_out_action])
        m.addSeparator()
        layout_menu = m.addMenu("La&yout")
        layout_menu.addActions(self.layout_actions.actions())
        m.addActions([self.multi_action, self.add_view_action, self.open_views_action, self.remove_view_action,
                      self.link_action])
        self.source_menu = m.addMenu("Source of this vie&w")
        self.source_menu.aboutToShow.connect(lambda: self._fill_source_menu(self.active, self.source_menu))
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
        panel.tool_changed.connect(lambda tool: [p.view.set_tool(tool) for p in self.panes])  # tools work on any view

        def redraw():
            for pane in self.panes:  # calibration may have changed for all
                selected = panel.selected_id() if pane is self.active else None
                pane.view.set_measurements(pane.measurements, panel.calibration, selected)

        panel.selection_changed.connect(lambda sel: self.active.view.set_selected(sel))
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
        panel.changed.connect(lambda: self.active.view.viewport().update())
        panel.export_requested.connect(self.export_counts)

        self.count_dock = self._add_dock("Counting", "count_dock", panel,
                                         Qt.DockWidgetArea.BottomDockWidgetArea, "Ctrl+4")
        self.count_dock.hide()  # same edge as Measurements; its stripe button opens it

    def _build_fiber_dock(self):
        panel = self.fiber_panel
        panel.find_requested.connect(self.find_fiber_axes)
        panel.method_changed.connect(self._fiber_settings_changed)
        panel.changed.connect(lambda: self.active.view.set_fiber_axes(panel.axes, panel.selected()))
        panel.export_requested.connect(self.export_fiber_axes)
        panel.live_changed.connect(lambda on: on and self.find_fiber_axes())
        panel.show_parts_check.toggled.connect(self._show_fiber_parts)
        self.fiber_dock = self._add_dock("Fiber axis", "fiber_dock", panel,
                                         Qt.DockWidgetArea.BottomDockWidgetArea, "Ctrl+5")
        self.fiber_dock.hide()  # same edge as Measurements; its stripe button opens it

    def _show_fiber_parts(self, on: bool):
        for pane in self.panes:
            pane.view.show_fiber_parts = on
            pane.view.viewport().update()

    def _build_gallery_dock(self):
        self.gallery.open_requested.connect(self.open_image_path)
        self.gallery.folder_changed.connect(self._update_next_name)
        self.gallery.process_requested.connect(self.run_process)
        self.gallery.open_views_requested.connect(self.open_paths_in_new_views)
        # On the left by default: each edge shows one panel at a time, and
        # Controls has the right.
        self.gallery_dock = self._add_dock("Captures", "gallery_dock", self.gallery,
                                           Qt.DockWidgetArea.LeftDockWidgetArea, "Ctrl+3")

    def _build_views(self):
        v = self.views
        v.add_requested.connect(self._add_view_and_choose)
        v.save_requested.connect(self.save_all_views)
        v.remove_requested.connect(lambda view: self.remove_view(self._pane_of_view(view)))
        v.source_menu_requested.connect(lambda view, menu: self._fill_source_menu(self._pane_of_view(view), menu))
        v.tab_clicked.connect(lambda view: self.set_active(self._pane_of_view(view)))
        v.layout_changed.connect(self._on_layout_changed)
        v.linked_changed.connect(self.link_action.setChecked)
        v.linked_changed.connect(lambda _on: self._update_capture_enabled())
        self.link_action.setChecked(v.link_check.isChecked())
        self._on_layout_changed(v.layout_name)

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
        # filled in when the camera opens and reports them (_update_controls).
        self.cam_box = QGroupBox("Camera")
        cam_layout = QVBoxLayout(self.cam_box)
        status_row = QHBoxLayout()
        self.camera_status = QLabel("Connecting...")
        status_row.addWidget(self.camera_status, stretch=1)
        reconnect = QPushButton("Reconnect")
        reconnect.setToolTip("Connect to the camera again, e.g. after turning it on or plugging it in.")
        reconnect.clicked.connect(self.reconnect_camera)
        status_row.addWidget(reconnect)
        cam_layout.addLayout(status_row)
        device_form = QFormLayout()
        device_form.setContentsMargins(0, 0, 0, 0)
        self.camera_combo = _DeviceCombo()
        self.camera_combo.setSizeAdjustPolicy(QComboBox.SizeAdjustPolicy.AdjustToMinimumContentsLengthWithIcon)
        self.camera_combo.setMinimumContentsLength(12)
        self.camera_combo.setToolTip("The camera this view shows. Any USB camera Windows lists will work;\n"
                                     "one in use in another view is moved to this one.")
        self.camera_combo.about_to_show.connect(self._fill_camera_combo)
        self.camera_combo.activated.connect(self._on_camera_chosen)
        device_form.addRow("Device", self.camera_combo)
        self.resolution_combo = QComboBox()
        self.resolution_combo.setToolTip("Frame size from the camera. The largest has the most detail;\n"
                                         "a smaller one may give more frames per second.")
        self.resolution_combo.setEnabled(False)  # until the camera opens
        self.resolution_combo.activated.connect(self._on_resolution_chosen)
        device_form.addRow("Resolution", self.resolution_combo)
        cam_layout.addLayout(device_form)
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
        layout.addWidget(self.cam_box)

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
            "highest number already in the folder, so nothing is overwritten.\n"
            "Capturing from several cameras at once (Link views) adds _view1, _view2, ..."
        )
        form.addRow("Name", self.name_edit)
        self.next_name_label = QLabel()
        self.next_name_label.setStyleSheet("color: gray;")
        form.addRow("", self.next_name_label)

        flat_button = self._tool_button(self.flat_live_action)
        flat_button.setToolTip("Correct uneven lighting in the live views, photos, videos and time-lapses.\n"
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
        for text, scale in SAVE_SCALES:
            self.size_combo.addItem(text, scale)
        self.size_combo.setToolTip(
            "Native saves frames at the size the camera sends. The HD2 size is\n"
            "scaled up 1.7x to match HD2 files (1920x1080 becomes 3264x1836):\n"
            "more pixels, no extra detail, and a different µm/px."
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
        row3 = QHBoxLayout()
        for a in (self.multi_action, self.add_view_action):
            row3.addWidget(self._tool_button(a))
        grid.addLayout(row1)
        grid.addLayout(row2)
        grid.addLayout(row3)
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
        # What the active view is showing: live, frozen, or which file.
        self.source_label = QLabel()
        self.source_label.setStyleSheet("font-weight: bold")
        for w in (self.cursor_label, self.zoom_label, self.size_label, self.fps_label, self.source_label):
            self.statusBar().addPermanentWidget(w)

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
        self.fiber_panel.set_method(s.value("fiber/method_v2", "auto"))
        self.fiber_panel.advanced_check.setChecked(s.value("fiber/advanced", False, type=bool))
        self.fiber_panel.centre_only_check.setChecked(s.value("fiber/centre_only", False, type=bool))
        self.fiber_panel.parts_centre_check.setChecked(s.value("fiber/parts_centre", False, type=bool))
        self.fiber_panel.show_parts_check.setChecked(s.value("fiber/show_parts", True, type=bool))
        try:
            self.fiber_panel.set_detection(Detection(**json.loads(s.value("fiber/detection", "{}"))))
        except (ValueError, TypeError):
            pass
        try:
            self.record_panel.set_state(json.loads(s.value("recording/state", "{}")))
        except (ValueError, TypeError):
            pass  # unreadable saved state: keep the defaults
        self.gallery.set_output_folder(self.folder_edit.text())
        self.gallery.set_follow_output(s.value("gallery/follow_output", True, type=bool),
                                       s.value("gallery/folder", ""))
        self._update_next_name()
        flat_path = s.value("flatfield/path", "")
        if flat_path and Path(flat_path).is_file():
            self._use_flat(flat_path, quiet=True)
            if s.value("flatfield/on", False, type=bool):
                self._set_flat_live(True)
        # Grid and crosshair (setChecked doesn't fire triggered, so the views are set after).
        self.grid_action.setChecked(s.value("view/grid", False, type=bool))
        self.crosshair_action.setChecked(s.value("view/crosshair", False, type=bool))
        for key, kind, name in (("view/grid_style", GridStyle, "grid_style"),
                                ("view/crosshair_style", CrosshairStyle, "crosshair_style")):
            try:
                setattr(self, name, kind.from_dict(json.loads(s.value(key, "{}"))))
            except (TypeError, ValueError, AttributeError):
                pass  # unreadable: keep the default look

        # The views and what each showed. Settings from before views existed
        # hold just the camera (camera/name, camera/n): one view with it.
        try:
            specs = json.loads(s.value("views/state", "")) if s.value("views/state", "") else None
        except (TypeError, ValueError):
            specs = None
        if not isinstance(specs, list) or not specs:
            specs = [{"camera": [s.value("camera/name"), int(s.value("camera/n", 0))]}] if s.value("camera/name", "") \
                else [{}]
        devices = self._refresh_devices()
        if self._cli_index is not None:  # main.py --index: that camera in the first view
            if self._cli_index < len(devices):
                specs[0] = {"camera": list(devices[self._cli_index])}
            self._cli_index = None
        if not s.value("views/state", "") and not specs[0] and devices:
            specs[0] = {"camera": list(devices[0])}  # first run: the first camera
        self.set_active(self._new_pane())
        self._restore_views(specs)
        self.views.set_linked(s.value("views/link", True, type=bool))
        self._set_layout(s.value("views/layout", "single"), add=False)
        self._apply_overlays()

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
        self._update_titles()
        self._update_controls()

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
        first = self.panes[0].camera if self.panes else None
        if first is not None and first.key:  # also read by versions from before views
            s.setValue("camera/name", first.key[0])
            s.setValue("camera/n", first.key[1])
        s.setValue("views/state", json.dumps(self._view_specs()))
        s.setValue("views/layout", self.views.layout_name)
        s.setValue("views/link", self.views.link_check.isChecked())
        s.setValue("capture/folder", self.folder_edit.text())
        s.setValue("capture/average_idx", self.average_combo.currentIndex())
        s.setValue("capture/size_idx", self.size_combo.currentIndex())
        s.setValue("capture/format_idx", self.format_combo.currentIndex())
        s.setValue("capture/tab", self.capture_tabs.currentIndex())
        s.setValue("capture/name", self.name_edit.text())
        s.setValue("gallery/count", self.gallery.count)
        s.setValue("gallery/follow_output", self.gallery.follows_output)
        s.setValue("gallery/folder", str(self.gallery.folder or ""))
        s.setValue("fiber/method_v2", self.fiber_panel.method())
        s.setValue("fiber/advanced", self.fiber_panel.advanced_check.isChecked())
        s.setValue("fiber/centre_only", self.fiber_panel.centre_only_check.isChecked())
        s.setValue("fiber/parts_centre", self.fiber_panel.parts_centre_check.isChecked())
        s.setValue("fiber/show_parts", self.fiber_panel.show_parts_check.isChecked())
        s.setValue("fiber/detection", json.dumps(asdict(self.fiber_panel.detection())))
        s.setValue("view/grid", self.grid_action.isChecked())
        s.setValue("view/grid_style", json.dumps(self.grid_style.to_dict()))
        s.setValue("view/crosshair_style", json.dumps(self.crosshair_style.to_dict()))
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

    # --- views ----------------------------------------------------------------------

    def _new_pane(self) -> Pane:
        """A new, empty view at the end."""
        view = ImageView()
        pane = Pane(view, [], Counter(names=self.count_panel.counter.names))
        self.panes.append(pane)
        self.views.add_view(view)
        self._connect_pane(pane)
        self._update_titles()
        return pane

    def _connect_pane(self, pane: Pane):
        """Hook a view up to the panels, and have it join in whatever is going on."""
        view = pane.view
        view.set_measurements(pane.measurements, self.measure_panel.calibration)
        view.activated.connect(lambda p=pane: self.set_active(p))
        view.measurement_drawn.connect(self._on_measurement_drawn)
        view.tool_exit_requested.connect(lambda: self.measure_panel.set_tool(None))
        view.set_counter(pane.counter)
        # The click made this view active first, so the panel holds its marks.
        view.count_add.connect(self.count_panel.add)
        view.count_remove.connect(self.count_panel.remove_near)
        view.count_undo.connect(self.count_panel.undo)
        view.tool_exit_requested.connect(lambda: self.count_panel.set_counting(False))
        view.cursor_moved.connect(self._on_cursor)
        view.zoom_changed.connect(lambda z, p=pane: p is self.active and self.zoom_label.setText(f"{z * 100:.0f}%"))
        if self.active is not None:  # a measuring tool or counting picked already
            view.set_tool(self.active.view.tool)
            if self.active.view.count_class is not None:
                view.set_count_class(self.active.view.count_class)
        view.show_fiber_parts = self.fiber_panel.show_parts_check.isChecked()
        view.set_overlays(self.grid_action.isChecked(), self.grid_style,
                          self.crosshair_action.isChecked(), self.crosshair_style)

    def _pane_of_view(self, view: ImageView) -> Pane | None:
        return next((p for p in self.panes if p.view is view), None)

    def _pane_of_link(self, link: CameraLink) -> Pane | None:
        return next((p for p in self.panes if p.camera is link), None)

    def _number(self, pane: Pane) -> int:
        return self.panes.index(pane) + 1

    def _targets(self) -> list[Pane]:
        """The views an action works on: all of them with Link views on, else the active one."""
        return list(self.panes) if self.views.linked else [self.active]

    def add_view(self) -> Pane | None:
        """Add an empty view (and make it the active one); its Source menu picks what it shows."""
        if self.views.is_full():
            self.statusBar().showMessage("That's as many views as fit. Remove one first.", 6000)
            return None
        pane = self._new_pane()
        if self.views.layout_name == "single":
            self._set_layout(self.views.multi_layout, add=False)
        self.set_active(pane)
        self.statusBar().showMessage(f"View {self._number(pane)} added: choose a camera or an image in its "
                                     "Source menu (or Controls > Device).", 8000)
        return pane

    def _add_view_and_choose(self):
        """Add a view and open its Source menu, to pick its camera or image straight away."""
        pane = self.add_view()
        if pane is not None:
            menu = QMenu(self)
            self._fill_source_menu(pane, menu)
            menu.exec(QCursor.pos())

    def open_in_new_views(self):
        paths, _ = QFileDialog.getOpenFileNames(self, "Open images in new views", self.folder_edit.text(), FILE_FILTER)
        if paths:
            self.open_paths_in_new_views(paths)

    def open_paths_in_new_views(self, paths: list[str]):
        """Each image in a new view (e.g. from Captures, to compare)."""
        failed, skipped, added = [], 0, []
        for path in paths:
            if self.views.is_full():
                skipped += 1
                continue
            img = load_image_file(path)
            if img is None:
                failed.append(Path(path).name)
                continue
            pane = self._new_pane()
            self._show_image(pane, img, path=path)
            added.append(pane)
        if added and self.views.layout_name == "single":
            self._set_layout(self.views.multi_layout, add=False)
        if failed:
            QMessageBox.warning(self, "Open in new views", "Could not read " + ", ".join(failed))
        if skipped:
            self.statusBar().showMessage(
                f"No room for {skipped} more view{'s' if skipped != 1 else ''}. Remove some first.", 8000)

    def remove_view(self, pane: Pane | None):
        """Take a view away, with its camera, measurements, marks and fiber axes."""
        if pane is None or pane not in self.panes:
            return
        if len(self.panes) == 1:
            self.statusBar().showMessage("There's always at least one view.", 5000)
            return
        if pane.camera is not None and not self._ok_to_stop(pane.camera, "Remove view"):
            return
        if pane is self.active:
            self.set_active(next(p for p in self.panes if p is not pane))
        if self._video_pane is pane:
            self._close_video()
        self._release_camera(pane)
        self._fiber_waiting.pop(pane, None)
        self.panes.remove(pane)
        self.views.remove_view(pane.view)
        self._choose_controlled()
        self._update_titles()
        self._update_controls()

    def _toggle_multi(self, on: bool):
        """Ctrl+K: one view / several views."""
        self._set_layout(self.views.multi_layout if on else "single")

    def _set_layout(self, name: str, add: bool = True):
        """add: going from one view to several adds an empty second one."""
        if name != "single" and len(self.panes) < 2 and add:
            self.views.set_layout(name)
            self._add_view_and_choose()
            return
        self.views.set_layout(name)
        self._on_layout_changed(self.views.layout_name)

    def _on_layout_changed(self, name: str):
        for a in self.layout_actions.actions():
            a.setChecked(a.data() == name)
        self.multi_action.setChecked(name != "single")

    def save_all_views(self):
        panes = [p for p in self.panes if p.view.image is not None]
        if len(panes) < 2:
            QMessageBox.information(self, "Save all views", "Two or more views need an image first.")
            return
        names = [f"{self._number(p)} {self._source_text(p)[0]}" for p in panes]
        default = next_capture_path(Path(self.folder_edit.text()), "views", "png")
        path, _ = QFileDialog.getSaveFileName(
            self, "Save all views as one image", str(default), "PNG (*.png);;TIFF (*.tif);;JPEG (*.jpg)")
        if not path:
            return
        path = Path(path)
        if path.suffix.lower() not in (".tif", ".tiff", ".png", ".jpg", ".jpeg"):
            path = path.with_suffix(".png")
        try:
            save_image_file(path, grid_image([p.view.image for p in panes], names))
        except Exception as e:
            QMessageBox.warning(self, "Save all views", f"Save failed: {e}")
            return
        self.statusBar().showMessage(f"Saved {path}", 5000)

    def _view_specs(self) -> list[dict]:
        """What each view shows, for saving: its camera, or its file."""
        specs = []
        for p in self.panes:
            if p.camera is not None and p.camera.key:
                specs.append({"camera": list(p.camera.key)})
            elif p.path and p.result is None and not is_video(p.path):
                specs.append({"path": os.path.abspath(p.path)})
            else:
                specs.append({})
        return specs

    def _restore_views(self, specs: list[dict]):
        """Make the views show what specs say (from saved settings or a settings
        file). Views beyond them are removed if nothing is lost by it."""
        for i, spec in enumerate(specs[:12]):
            if not isinstance(spec, dict):
                continue
            pane = self.panes[i] if i < len(self.panes) else self._new_pane()
            key, path = spec.get("camera"), spec.get("path")
            if isinstance(key, list) and len(key) == 2 and key[0]:
                key = (str(key[0]), int(key[1]))
                if pane.camera is None or pane.camera.key != key:
                    self._set_pane_camera(pane, key, ask=False)
            elif isinstance(path, str) and path and Path(path).is_file() and pane.path != path:
                img = load_image_file(path)
                if img is not None:
                    self._show_image(pane, img, path=path)
        for pane in self.panes[len(specs):]:
            busy = pane.camera is not None and self._busy(pane.camera)
            if not (busy or pane.measurements or pane.counter.marks or pane.fiber_axes):
                self.remove_view(pane)

    # --- sources ------------------------------------------------------------------------

    def _refresh_devices(self) -> list[tuple[str, int]]:
        """Read the camera list from Windows (kept: it takes a moment)."""
        self._devices = camera_list()
        return self._devices

    def _camera_owner(self, key) -> Pane | None:
        return next((p for p in self.panes if p.camera is not None and p.camera.key == key), None)

    def _fill_source_menu(self, pane: Pane | None, menu: QMenu):
        """A view's Source menu: its camera, No camera, Live, Open image, Remove."""
        menu.clear()
        if pane is None:
            return
        devices = self._refresh_devices()
        current = pane.camera.key if pane.camera is not None else None
        menu.addSection("Camera")
        for key in devices + ([current] if current and current not in devices else []):
            text = camera_label(key)
            owner = self._camera_owner(key)
            if owner is not None and owner is not pane:
                text += f"  (in view {self._number(owner)}: move it here)"
            if key not in devices:
                text += "  (not connected)"
            a = menu.addAction(text, lambda k=key: self._set_pane_camera(pane, k))
            a.setCheckable(True)
            a.setChecked(key == current)
        if not devices:
            menu.addAction("No cameras found").setEnabled(False)
        a = menu.addAction("No camera", lambda: self._set_pane_camera(pane, None))
        a.setCheckable(True)
        a.setChecked(current is None)
        menu.addSeparator()
        live = menu.addAction("Live", lambda on: self._set_live(pane, on))
        live.setCheckable(True)
        live.setChecked(pane.live)
        live.setEnabled(pane.camera is not None)
        menu.addAction("Open image...", lambda: self.open_image(pane))
        menu.addSeparator()
        menu.addAction("Remove this view", lambda: self.remove_view(pane)).setEnabled(len(self.panes) > 1)

    def _set_pane_camera(self, pane: Pane, key, ask: bool = True) -> bool:
        """Show camera key in pane, live (None: no camera). A camera used by
        another view moves to this one. False if the user chose to keep a
        recording going instead."""
        current = pane.camera.key if pane.camera is not None else None
        if key == current:
            if pane.camera is not None:
                self._set_live(pane, True)
            return True
        owner = self._camera_owner(key) if key is not None else None
        for link in (pane.camera, owner.camera if owner else None):
            if link is not None and ask and not self._ok_to_stop(link, "Change camera"):
                return False
        if owner is not None:
            self._release_camera(owner)
            self._update_source(owner)
        self._release_camera(pane)
        if key is not None:
            self._attach_camera(pane, key)
            self._set_live(pane, True)
        self._choose_controlled()
        self._update_controls()
        self._update_source(pane)
        return True

    def _attach_camera(self, pane: Pane, key: tuple[str, int]):
        link = CameraLink(key, self)
        link.opened.connect(self._on_camera_opened)
        link.open_failed.connect(self._on_open_failed)
        link.frame_ready.connect(self._on_frame)
        link.settings_changed.connect(self._on_camera_settings)
        link.capture_started.connect(self._on_capture_started)
        link.capture_done.connect(self._on_capture_done)
        link.error.connect(self._on_error)
        link.recording_changed.connect(self._on_recording_changed)
        link.recording_progress.connect(
            lambda lk, s, f: lk is self._link() and self.record_panel.set_record_progress(s, f))
        link.timelapse_progress.connect(self._on_timelapse_progress)
        link.timelapse_finished.connect(self._on_timelapse_finished)
        pane.camera = link
        self._start_link(link)

    def _start_link(self, link: CameraLink):
        devices = self._refresh_devices()
        index = devices.index(link.key) if link.key in devices else None  # not plugged in: the worker says so
        link.start(index, self._saved_resolution(link.key), self.flat if self.flat_live_action.isChecked() else None)

    def _release_camera(self, pane: Pane):
        """Close pane's camera; it keeps showing the last frame."""
        link = pane.camera
        if link is None:
            return
        pane.camera = None
        pane.live = False
        self._capturing.discard(link)
        link.stop()
        link.deleteLater()
        if self.controlled is pane:
            self.controlled = None
            self._choose_controlled()

    def _busy(self, link: CameraLink) -> list[str]:
        return [what for what, on in (("a video is recording", link.recording),
                                      ("a time-lapse is running", link.timelapse)) if on]

    def _ok_to_stop(self, link: CameraLink, title: str) -> bool:
        """True if link's camera isn't recording, or the user agrees to stop it."""
        busy = self._busy(link)
        return not busy or QMessageBox.question(
            self, title, f"On {camera_label(link.key)}, {' and '.join(busy)}. Stop it?"
        ) == QMessageBox.StandardButton.Yes

    # --- which camera Controls work on ----------------------------------------------------

    def _choose_controlled(self):
        """Controls work on the active view's camera; if it has none, on the
        camera they had; else on the first view with one."""
        if self.active is not None and self.active.camera is not None:
            new = self.active
        elif self.controlled in self.panes and self.controlled.camera is not None:
            new = self.controlled
        else:
            new = next((p for p in self.panes if p.camera is not None), None)
        if new is not self.controlled:
            self.controlled = new
            self.record_panel.record_status.setText("")
            self._update_controls()

    def _link(self) -> CameraLink | None:
        return self.controlled.camera if self.controlled is not None else None

    def _update_controls(self):
        """Show the controlled camera in the Camera box, Resolution, sliders and recording."""
        if not hasattr(self, "_devices"):
            return  # still building the window
        link = self._link()
        title = "Camera"
        if self.controlled is not None and len(self.panes) > 1:
            title += f" (view {self._number(self.controlled)})"
        self.cam_box.setTitle(title)
        if link is None:
            self._set_camera_status("No camera in this view", "color: gray;",
                                    "Choose one in Device, or in a view's Source menu.")
        else:
            self._set_camera_status(*link.status)
        self._fill_camera_combo(refresh=False)
        self._fill_resolution_combo()
        is_open = link is not None and link.open
        for name, slider in self.sliders.items():
            r = link.ranges.get(name) if is_open else None
            slider.setEnabled(r is not None)
            if r is None:
                self.value_labels[name].setText("not available" if is_open else "-")
                continue
            slider.blockSignals(True)
            slider.setRange(r["min"], r["max"])
            if name in link.values:
                slider.setValue(link.values[name])
            slider.blockSignals(False)
            if name in link.values:
                self._show_value(name, link.values[name])
        self.slider_box.setEnabled(is_open)
        self.record_panel.set_enabled(is_open)
        recording = link is not None and link.recording
        if self.record_panel.record_button.isChecked() != recording:
            self.record_panel.set_recording(recording, "")
        timelapse = link is not None and link.timelapse
        if self.record_panel.tl_button.isChecked() != timelapse:
            self.record_panel.set_timelapse_running(timelapse)
        self._update_save_sizes()
        self._update_capture_enabled()

    def _set_camera_status(self, text: str, style: str = "", tooltip: str = ""):
        self.camera_status.setText(text)
        self.camera_status.setStyleSheet(style)
        self.camera_status.setToolTip(tooltip)

    def _fill_camera_combo(self, refresh: bool = True):
        """List the cameras in the Device box: the controlled one selected, ones
        in other views marked, and No camera."""
        devices = self._refresh_devices() if refresh else self._devices
        target = self.controlled or self.active
        current = target.camera.key if target is not None and target.camera is not None else None
        combo = self.camera_combo
        combo.blockSignals(True)
        combo.clear()
        for key in devices + ([current] if current and current not in devices else []):
            text = camera_label(key)
            owner = self._camera_owner(key)
            if owner is not None and owner is not target:
                text += f" (view {self._number(owner)})"
            if key not in devices:
                text += " (not connected)"
            combo.addItem(text, key)
        combo.addItem("No camera", NO_CAMERA)
        combo.setCurrentIndex(find_item(combo, current if current else NO_CAMERA))
        combo.blockSignals(False)

    def _on_camera_chosen(self, _index: int):
        key = self.camera_combo.currentData()
        key = None if key is None or tuple(key) == NO_CAMERA else tuple(key)
        target = self.controlled or self.active
        if target is None or not self._set_pane_camera(target, key):
            self._fill_camera_combo(refresh=False)

    def _resolution_setting(self, key) -> str:
        name, n = key
        return "camera/resolution/" + re.sub(r"[^\w.-]", "_", f"{name}_{n}")

    def _saved_resolution(self, key) -> tuple[int, int] | None:
        """The frame size chosen for this camera, or None for its largest."""
        if not key:
            return None
        m = re.fullmatch(r"(\d+)x(\d+)", str(self.settings.value(self._resolution_setting(key), "")))
        return (int(m[1]), int(m[2])) if m else None

    def _on_resolution_chosen(self, _index: int):
        link = self._link()
        size = self.resolution_combo.currentData()
        if link is None or size is None or tuple(size) == link.frame_size:
            return
        self.settings.setValue(self._resolution_setting(link.key), f"{size[0]}x{size[1]}")
        if not self.reconnect_camera():
            self._fill_resolution_combo()

    def _fill_resolution_combo(self):
        """List the controlled camera's frame sizes in the Resolution box, the open one selected."""
        link = self._link()
        combo = self.resolution_combo
        combo.blockSignals(True)
        combo.clear()
        sizes = frame_sizes(link.modes) if link is not None else []
        size = link.frame_size if link is not None else None
        if size and size not in [s[:2] for s in sizes]:
            sizes.append((*size, None))
        for w, h, fps in sizes:
            combo.addItem(f"{w} x {h}" + (f"  ({fps:g} fps)" if fps else ""), (w, h))
        if size:
            combo.setCurrentIndex(find_item(combo, size))
        combo.setEnabled(link is not None and link.open and len(sizes) > 1)
        combo.blockSignals(False)

    def _update_save_sizes(self):
        """Show the real save sizes for the controlled camera in the Save size box."""
        link = self._link()
        size = link.frame_size if link is not None else None
        for i, (text, scale) in enumerate(SAVE_SCALES):
            if size:
                w, h = scaled_size(*size, scale)
                text = f"{w} x {h} (native)" if scale == 1 else f"{w} x {h} (HD2 size, 1.7x upscaled)"
            self.size_combo.setItemText(i, text)

    def reconnect_camera(self) -> bool:
        """Close the controlled camera (if open) and open it again. False if
        the user chose not to stop a recording."""
        link = self._link()
        if link is None:
            self.statusBar().showMessage("This view has no camera: choose one in Device.", 5000)
            return False
        if not self._ok_to_stop(link, "Reconnect camera"):
            return False
        self.statusBar().showMessage("Reconnecting to the camera...")
        self._capturing.discard(link)
        self._start_link(link)
        self._update_controls()
        self._update_source(self.controlled)
        return True

    # --- camera events ------------------------------------------------------------

    def _on_camera_opened(self, link: CameraLink, info: dict):
        pane = self._pane_of_link(link)
        if link is self._link():
            self._update_controls()
            self.statusBar().showMessage("Camera open", 3000)
        # Values from a settings file, waiting for this camera.
        values = self._camera_values.pop(camera_label(link.key), None)
        if values:
            self._apply_camera_values(link, values)
        self._update_capture_enabled()
        self._update_source(pane)

    def _on_open_failed(self, link: CameraLink, message: str):
        if link is self._link():
            self._update_controls()
            self.statusBar().showMessage(f"{message} Turn the camera on, then click Reconnect.")
        self._update_source(self._pane_of_link(link))

    def _on_frame(self, link: CameraLink, frame):
        pane = self._pane_of_link(link)
        if pane is None or not pane.live:
            return
        pane.view.set_image(frame)
        if pane is self.active:
            fps = link.fps()
            if fps:
                self.fps_label.setText(f"{fps:.1f} fps")
            h, w = frame.shape[:2]
            self.size_label.setText(f"{w} x {h}")
        # Fiber axes of the live views, a few times a second, taking turns, in
        # the background so the video doesn't stall. Frames that come while
        # it's busy are skipped.
        if self.fiber_panel.live_check.isChecked() and self._fiber_job is None:
            now = time.perf_counter()
            live = [p for p in self.panes if p.live]
            if now - pane.fiber_last >= FIBER_LIVE_INTERVAL_S and pane.fiber_last <= min(p.fiber_last for p in live):
                pane.fiber_last = now
                self._request_fibers(pane, frame.copy(), "live")  # copy: the worker may reuse its buffer

    def _on_camera_settings(self, link: CameraLink, values: dict):
        if link is not self._link():
            return
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

    def _on_capture_started(self, link: CameraLink, n_frames: int, seconds: float):
        self.statusBar().showMessage(f"Capturing {n_frames} frame(s), about {seconds:.1f} s...")

    def _on_capture_done(self, link: CameraLink, path: str):
        self._capturing.discard(link)
        self._update_capture_enabled()
        self.last_saved_label.setText(f"Saved {Path(path).name}")
        self.statusBar().showMessage(f"Saved {path}", 5000)
        self._update_next_name()
        # The folder watcher would pick it up too; this also selects it.
        self.gallery.refresh(select=path)

    def _on_error(self, link: CameraLink, message: str):
        self._capturing.discard(link)
        self._update_capture_enabled()
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
        link = self._link()
        if link is not None and link.worker is not None:
            link.worker.set_control(name, value)

    def _capture_links(self) -> list[CameraLink]:
        """The cameras Capture takes from: every open one with Link views on, else the controlled one."""
        if self.views.linked:
            return [p.camera for p in self.panes if p.camera is not None and p.camera.open]
        link = self._link()
        return [link] if link is not None and link.open else []

    def _update_capture_enabled(self):
        if hasattr(self, "_capturing"):
            self.capture_action.setEnabled(bool(self._capture_links()) and not self._capturing)

    def capture(self):
        links = self._capture_links()
        if not links or self._capturing:
            return
        name = clean_name(self.name_edit.text())
        for link in links:
            this_name = name
            if len(links) > 1:  # each camera its own series, so two can't take the same number
                this_name = f"{name or 'cap'}_view{self._number(self._pane_of_link(link))}"
            self._capturing.add(link)
            link.worker.capture(self.average_combo.currentData(), self.size_combo.currentData(),
                                self.format_combo.currentData(), Path(self.folder_edit.text()), this_name)
        self._update_capture_enabled()

    # --- live / freeze and what each view shows ------------------------------------------

    def _toggle_live(self, on: bool):
        """L: live or frozen, for the active view (all of them with Link views on)."""
        for pane in self._targets():
            if pane.camera is not None:
                self._set_live(pane, on)
        if self.active is not None:
            self.live_action.setChecked(self.active.live)

    def _set_live(self, pane: Pane, on: bool):
        on = on and pane.camera is not None
        pane.live = on
        if on:
            pane.path, pane.title, pane.result = None, "", None
            pane.fiber_last = 0.0
            if self._video_pane is pane:
                self._close_video()
        self._update_source(pane)

    def _source_text(self, pane: Pane) -> tuple[str, str]:
        """What pane shows, for its title and the status bar: (text, tooltip)."""
        cam = camera_label(pane.camera.key) if pane.camera is not None and pane.camera.key else None
        if pane.live:
            if pane.camera.open:
                return f"{cam} · Live", pane.camera.name
            text, _, tip = pane.camera.status
            return f"{cam} · {text}", tip
        if pane.result is not None:
            return f"{pane.title} (unsaved)", "Processing result. File > Save image as (Ctrl+S) to keep it."
        if pane.path:
            return Path(pane.path).name, pane.path
        if pane.view.image is not None:
            return "Frozen" + (f" ({cam})" if cam else ""), "L goes back to live." if cam else ""
        return "Empty: choose a camera or an image in Source", ""

    def _update_source(self, pane: Pane | None):
        if pane is None or pane not in self.panes:
            return
        text, tip = self._source_text(pane)
        self.views.set_title(pane.view, text, tip)
        if pane is not self.active:
            return
        self.source_label.setText(text)
        self.source_label.setToolTip(tip)
        title = f"CamControl - {text}"
        if len(self.panes) > 1:
            title += f"  (view {self._number(pane)})"
        if self.settings_path:
            title += f"   [{Path(self.settings_path).stem}]"
        self.setWindowTitle(title)
        self.gallery.set_open_path(pane.path)  # framed in Captures
        self.live_action.setChecked(pane.live)
        self.live_action.setEnabled(pane.camera is not None)
        if pane.view.image is not None:
            h, w = pane.view.image.shape[:2]
            self.size_label.setText(f"{w} x {h}")
        else:
            self.size_label.clear()
        if not pane.live:
            self.fps_label.clear()

    def _update_titles(self):
        for pane in self.panes:
            self._update_source(pane)

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
            self.gallery.set_output_folder(folder)
            self._update_next_name()

    def open_capture_folder(self):
        folder = Path(self.folder_edit.text())
        folder.mkdir(parents=True, exist_ok=True)
        os.startfile(folder)

    def open_image(self, pane: Pane | None = None):
        path, _ = QFileDialog.getOpenFileName(
            self, "Open image or video", self.folder_edit.text(),
            "Images and videos (*.tif *.tiff *.png *.jpg *.jpeg *.bmp *.mp4 *.avi);;All files (*)",
        )
        if path:
            self.open_image_path(path, pane)

    def open_image_path(self, path: str, pane: Pane | None = None):
        """Show an image file in the active view (or pane); its camera keeps
        running for when it goes live again. Videos play."""
        pane = pane if pane in self.panes else self.active
        if is_video(path):
            self.open_video_path(path, pane)
            return
        img = load_image_file(path)
        if img is None:
            QMessageBox.warning(self, "CamControl", f"Could not read {path}")
            return
        self._ask_clear_annotations(pane, "Open image")
        if self._video_pane is pane:
            self._close_video()
        self._show_image(pane, img, path=path)

    def _show_image(self, pane: Pane, image, path: str | None = None, title: str = "", result: dict | None = None):
        """Show an image in pane, frozen: a file (path) or a processing result."""
        self._set_live(pane, False)
        pane.view.set_image(image)
        pane.path, pane.result = path, result
        pane.title = title or (Path(path).name if path else "")
        self._update_source(pane)
        self._measure_opened_image(pane)

    def open_video_path(self, path: str, pane: Pane | None = None):
        """Open a video in the active view (or pane), with the player bar under the views."""
        pane = pane if pane in self.panes else self.active
        self._ask_clear_annotations(pane, "Open video")
        self._set_live(pane, False)
        self._close_video()  # one video plays at a time
        self._video_pane = pane
        if not self.player.open(path):  # shows the first frame via _on_video_frame
            self._close_video()
            QMessageBox.warning(self, "CamControl", f"Could not play {path}")
            return
        self.player.show()
        pane.path, pane.title, pane.result = path, Path(path).name, None
        self._update_source(pane)
        self.player.play()

    def _on_video_frame(self, frame):
        pane = self._video_pane
        if pane is None or pane not in self.panes:
            return
        pane.view.set_image(frame)
        if pane is self.active:
            h, w = frame.shape[:2]
            self.size_label.setText(f"{w} x {h}")

    def _close_video(self):
        self.player.close_video()
        self.player.hide()
        self._video_pane = None

    def _close_video_and_resume(self):
        """The player's close button: back to the camera, if the view has one."""
        pane = self._video_pane
        self._close_video()
        if pane is not None and pane.camera is not None:
            self._set_live(pane, True)

    # --- which view the panels work on --------------------------------------------

    def _active_source(self) -> str:
        """Name of what the active view shows, for exports."""
        pane = self.active
        return Path(pane.path).name if pane.path else self._source_text(pane)[0]

    def set_active(self, pane: Pane | None):
        """Make the panels show (and work on) this view."""
        if pane is None or pane is self.active or pane not in self.panes:
            return
        old = self.active
        if old is not None:
            old.next_id = self.measure_panel.next_id
            old.fiber_axes = self.fiber_panel.axes
            old.view.set_selected(None)
            old.view.set_fiber_axes(old.fiber_axes)
        self.active = pane
        self.measure_panel.set_state(pane.measurements, pane.next_id)
        self.count_panel.set_counter(pane.counter)
        self.fiber_panel.set_axes(pane.fiber_axes)
        self.views.set_active(pane.view)
        self.zoom_label.setText(f"{pane.view.zoom * 100:.0f}%")
        self._choose_controlled()
        self._update_source(pane)
        self._update_capture_enabled()
        if old is not None and len(self.panes) > 1:
            self.statusBar().showMessage(f"Working on view {self._number(pane)}: {self._source_text(pane)[0]}", 4000)

    def _ask_clear_annotations(self, pane: Pane, title: str):
        """Measurements and counts belong to the image they were made on."""
        self.set_active(pane)  # the view whose image is changing
        if not (self.measure_panel.measurements or self.count_panel.counter.marks):
            return
        if QMessageBox.question(
            self, title, "Clear this view's measurements and counts? They were made on a different image."
        ) == QMessageBox.StandardButton.Yes:
            self.measure_panel.clear(confirm=False)
            self.count_panel.clear(confirm=False)

    def save_image(self):
        """Save the active view's image: a processing result, a frozen frame or a copy of a file."""
        pane = self.active
        image = pane.view.image
        if image is None:
            return
        stem = pane.result["short_name"] if pane.result else "image"
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
        if pane.result:
            meta.update(pane.result["meta"])
        else:
            meta["source"] = self._active_source()
            if self._video_pane is pane and self.player.is_open:
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
        if pane.result:  # it's a file now
            pane.result, pane.path, pane.title = None, str(path), path.name
            self._update_source(pane)
        self.gallery.refresh(select=str(path))

    # --- processing -----------------------------------------------------------------

    def show_result(self, image, title: str, short_name: str, meta: dict):
        """Show a processing result in the active view, frozen and unsaved."""
        pane = self.active
        self._ask_clear_annotations(pane, title)
        if self._video_pane is pane:
            self._close_video()
        self._show_image(pane, image, title=title, result={"short_name": short_name, "meta": meta})
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

    def _links(self) -> list[CameraLink]:
        return [p.camera for p in self.panes if p.camera is not None]

    def _use_flat(self, path: str, quiet: bool = False) -> bool:
        img = load_image_file(path)
        if img is None:
            if not quiet:
                QMessageBox.warning(self, "Flat-field", f"Could not read {path}")
            return False
        self.flat = FlatField(img, Path(path).name)
        self._flat_path = str(path)
        if self.flat_live_action.isChecked():
            for link in self._links():  # swap in the new reference
                if link.worker is not None:
                    link.worker.set_flat_field(self.flat)
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
        """Save the active view's image as the flat reference and use it."""
        image = self.active.view.image
        if image is None:
            return
        if self.active.live and self.flat_live_action.isChecked():
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
        """Correct every camera's frames (one reference for all)."""
        on = on and self.flat is not None
        self.flat_live_action.setChecked(on)
        for link in self._links():
            if link.worker is not None:
                link.worker.set_flat_field(self.flat if on else None)

    def apply_flat_to_view(self):
        pane = self.active
        image = pane.view.image
        if image is None or self.flat is None:
            return
        if pane.live:
            QMessageBox.information(self, "Flat-field",
                                    "This corrects a frozen or opened image. For the live view, "
                                    "turn on \"Correct live view and captures\".")
            return
        name = self._active_source()
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
        if not panel.measurements or self.active.view.image is None:
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
        image = self.active.view.image
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
        if not counter.marks or self.active.view.image is None:
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
        image = self.active.view.image
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

    # --- fiber axes -----------------------------------------------------------------

    def _request_fibers(self, pane: Pane, image, kind: str):
        """Measure the fiber axes of pane's image in the background, with the
        panel's settings as they are now. kind: "live" (a camera frame;
        skipped while another measuring runs), "image" (an image opened, or the
        settings changed) or "find" (Find axes: say how many were found).
        One runs at a time; a request made meanwhile waits, replacing any other
        waiting one for the same view, so the newest image and settings win."""
        request = (pane, image, kind, pane.live, self._fiber_generation, self._fiber_analysis(pane))
        if self._fiber_job is not None:
            if kind != "live":
                self._fiber_waiting[pane] = request
            return
        analyse = request[-1]
        job = Job(lambda: analyse(image), self)
        job.succeeded.connect(lambda axes: self._fibers_done(request, axes))
        job.failed.connect(lambda msg: self.statusBar().showMessage(f"Fiber axes: {msg}", 6000))
        job.finished.connect(self._fibers_finished)
        self._fiber_job = job
        job.start(QThread.Priority.LowPriority)  # the video and the window come first

    def _fibers_done(self, request, axes):
        pane, image, kind, from_camera, generation, _ = request
        if generation != self._fiber_generation or pane not in self.panes:
            return  # measured with old settings (the new ones are being measured), or the view is gone
        # Only if the image measured is still the one shown: for a camera frame, while
        # the camera still shows (and, live updates, while they're on).
        if from_camera:
            if not pane.live or (kind == "live" and not self.fiber_panel.live_check.isChecked()):
                return
        elif pane.view.image is not image or pane.live:
            return
        self._show_fiber_axes(pane, axes)
        if kind == "find":
            if axes:
                unclear = sum(not f.clear for f in axes)
                note = f" ({unclear} unclear)" if unclear else ""
                self.statusBar().showMessage(f"Found {len(axes)} fiber{'s' if len(axes) != 1 else ''}{note}.", 6000)
            else:
                self.statusBar().showMessage(
                    "No fibers found. Draw a Circle (3 pt or centre) round the fiber in Measurements, then Find axes.", 10000)

    def _fibers_finished(self):
        self._fiber_job.deleteLater()
        self._fiber_job = None
        if self._fiber_waiting:
            pane = next(iter(self._fiber_waiting))
            _, image, kind = self._fiber_waiting.pop(pane)[:3]
            if pane in self.panes:
                self._request_fibers(pane, image, kind)  # with the settings as they are now

    def _fiber_settings_changed(self):
        """Method or detection settings changed: re-measure what's shown (where it was measured)."""
        self._fiber_generation += 1
        live_updates = self.fiber_panel.live_check.isChecked()
        for pane in self.panes:
            has_axes = self.fiber_panel.axes if pane is self.active else pane.fiber_axes
            image = pane.view.image
            if pane.live:
                if live_updates:
                    pane.fiber_last = 0.0  # its next frame
                elif has_axes and image is not None:
                    self._request_fibers(pane, image.copy(), "image")
            elif (has_axes or live_updates) and image is not None:
                self._request_fibers(pane, image, "image")

    def _measure_opened_image(self, pane: Pane):
        """A new image (a file, a result) was opened in pane: clear the old axes,
        and with Update live on measure it, in the background, so clicking
        through images keeps the axes up to date."""
        if pane.live or pane.view.image is None:
            return
        self._show_fiber_axes(pane, [])  # the old image's axes don't belong to this one
        if self.fiber_panel.live_check.isChecked():
            self._request_fibers(pane, pane.view.image, "image")

    def _fiber_analysis(self, pane: Pane):
        """A function measuring the fiber axes of an image with pane's circles
        and the panel's settings as they are now. Safe to call from another thread."""
        circles = None
        if self.fiber_panel.use_circles.isChecked():
            # Fibers marked with Circle measurements (in pixels, whatever the calibration).
            circles = []
            for m in pane.measurements:
                if m.kind in CIRCLE_KINDS:
                    r = compute(m)
                    circles.append((*r["center_px"], r["radius"]))
        method = self.fiber_panel.method()
        parts_centre = self.fiber_panel.parts_centre_check.isChecked()
        detection = self.fiber_panel.detection()
        centre_only = self.fiber_panel.centre_only_check.isChecked()
        return lambda image: analyse_fibers(image, circles or None, method=method, parts_centre=parts_centre,
                                            detection=detection, centre_only=centre_only)

    def _show_fiber_axes(self, pane: Pane, axes: list):
        if pane is self.active:
            self.fiber_panel.set_axes(axes)
        else:  # e.g. live updates of one view while another is active
            pane.fiber_axes = axes
            pane.view.set_fiber_axes(axes)

    def find_fiber_axes(self, quiet: bool = False):
        """Measure the fiber axes in the active view (every view with Link views on)."""
        found_any = False
        for pane in self._targets():
            image = pane.view.image
            if image is None:
                continue
            if pane.live:
                image = image.copy()  # the camera worker may reuse the frame's buffer
            self._request_fibers(pane, image, "find" if pane is self.active and not quiet else "image")
            found_any = True
        if found_any and not quiet:
            self.statusBar().showMessage("Finding fiber axes...")
            if self.fiber_dock.isHidden():  # show the results (minimizing the others on its edge)
                self.fiber_dock.toggleViewAction().trigger()

    def export_fiber_axes(self):
        """Save the fiber table (Excel or CSV) plus the image and a copy with the axes drawn."""
        panel = self.fiber_panel
        if not panel.axes or self.active.view.image is None:
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
        image = self.active.view.image
        try:
            table = pd.DataFrame(panel.rows(self._active_source()))
            if path.suffix.lower() == ".xlsx":
                table.to_excel(path, index=False, sheet_name="Fiber axes")
            else:
                table.to_csv(path, index=False, encoding="utf-8-sig")
            image_path = path.with_name(f"{path.stem}_image.png")
            drawn_path = path.with_name(f"{path.stem}_axes.png")
            to_qimage(image).save(str(image_path))
            render_fiber_axes(image, panel.axes, panel.show_parts_check.isChecked()).save(str(drawn_path))
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
                "save_scale": self.size_combo.currentData(),
                "format": self.format_combo.currentData(),
            },
            "output": {"folder": self.folder_edit.text(), "name": self.name_edit.text()},
            "flat_field": {"path": self._flat_path, "on": self.flat_live_action.isChecked()},
            "recording": self.record_panel.state(),
            "view": {"grid": self.grid_action.isChecked(), "grid_style": self.grid_style.to_dict(),
                     "crosshair": self.crosshair_action.isChecked(), "crosshair_style": self.crosshair_style.to_dict()},
            "views": {"layout": self.views.layout_name, "link": self.views.link_check.isChecked(),
                      "views": self._view_specs()},
        }
        # Each camera's control values (what it last reported, or what's waiting for it).
        cameras = dict(self._camera_values)
        for link in self._links():
            if link.key and link.values:
                cameras[camera_label(link.key)] = {n: v for n, v in link.values.items() if n in self.sliders}
        if cameras:
            data["cameras"] = cameras
        link = self._link()  # also as "camera", as files from before views had it
        if link is not None and link.key and camera_label(link.key) in cameras:
            data["camera"] = cameras[camera_label(link.key)]
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
        self._update_source(self.active)

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
        views = data.get("views")
        if isinstance(views, dict):
            if isinstance(views.get("views"), list) and views["views"]:
                self._restore_views(views["views"])
            if "link" in views:
                self.views.set_linked(bool(views["link"]))
            if views.get("layout") in LAYOUTS:
                self._set_layout(views["layout"], add=False)

        # Camera values: per camera name, or (files from before views) one set for the controlled camera.
        cameras = data.get("cameras")
        if not isinstance(cameras, dict):
            link = self._link()
            cameras = {camera_label(link.key): data["camera"]} if data.get("camera") and link is not None \
                and link.key else {}
        waiting = False
        for label, values in cameras.items():
            if not isinstance(values, dict):
                continue
            link = next((lk for lk in self._links() if lk.key and camera_label(lk.key) == label), None)
            if link is not None and link.open:
                self._apply_camera_values(link, values)
            else:
                self._camera_values[label] = values  # applied when that camera opens
                waiting = waiting or link is not None
        if waiting:
            problems.append("Camera settings will be applied when the camera opens.")

        cap = data.get("capture", {})
        scale = cap.get("save_scale")
        if scale is None and "save_size" in cap:  # files saved before 2026-10-06 held a size
            scale = HD2_SCALE if tuple(cap["save_size"]) == (3264, 1836) else 1.0
        for combo, value in ((self.average_combo, cap.get("average")),
                             (self.size_combo, scale),
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
            self.gallery.set_output_folder(out["folder"])
        if "name" in out:
            self.name_edit.setText(out["name"])
        self._update_next_name()

        view = data.get("view", {})
        for action, key in ((self.grid_action, "grid"), (self.crosshair_action, "crosshair")):
            if key in view:
                action.setChecked(bool(view[key]))
        if isinstance(view.get("grid_style"), dict):
            self.grid_style = GridStyle.from_dict(view["grid_style"])
        if isinstance(view.get("crosshair_style"), dict):
            self.crosshair_style = CrosshairStyle.from_dict(view["crosshair_style"])
        self._apply_overlays()
        return problems

    def _apply_camera_values(self, link: CameraLink, values: dict):
        for name, value in values.items():
            if name not in self.sliders or link.ranges.get(name) is None:
                continue
            if link is self._link():
                self.sliders[name].setValue(int(value))  # sends it to the camera via _on_slider
            elif link.worker is not None:
                link.worker.set_control(name, int(value))

    # --- grid and crosshair ---------------------------------------------------------------

    def show_overlay_settings(self, tab: str = "grid"):
        """The Grid and crosshair window (non-modal; the overlays follow its changes),
        on tab "grid" or "crosshair". Turns that overlay on."""
        (self.crosshair_action if tab == "crosshair" else self.grid_action).setChecked(True)
        if self._overlay_dialog is None:
            self._overlay_dialog = OverlayDialog(self.grid_style, self.crosshair_style, self)
            self._overlay_dialog.changed.connect(self._set_overlay_styles)
        self._apply_overlays()
        self._overlay_dialog.show_tab(tab)
        self._overlay_dialog.show()
        self._overlay_dialog.raise_()
        self._overlay_dialog.activateWindow()

    def _set_overlay_styles(self, grid: GridStyle, crosshair: CrosshairStyle):
        self.grid_style, self.crosshair_style = grid, crosshair
        self._apply_overlays()

    def _apply_overlays(self):
        """Grid and crosshair, as the View menu and the settings say, on every view."""
        for pane in self.panes:
            pane.view.set_overlays(self.grid_action.isChecked(), self.grid_style,
                                   self.crosshair_action.isChecked(), self.crosshair_style)
        dialog = self._overlay_dialog
        if dialog is not None and dialog.styles() != (self.grid_style, self.crosshair_style):
            dialog.set_styles(self.grid_style, self.crosshair_style)  # e.g. a settings file was opened

    # --- video and time-lapse -----------------------------------------------------------

    def _request_recording(self, start: bool):
        link = self._link()
        if link is None or link.worker is None:
            return
        if start:
            link.worker.start_recording(Path(self.folder_edit.text()), clean_name(self.name_edit.text()),
                                        **self.record_panel.video_settings())
        else:
            link.worker.stop_recording()

    def _on_recording_changed(self, link: CameraLink, on: bool, path: str):
        if link is self._link():
            self.record_panel.set_recording(on, path)
        if on:
            self.statusBar().showMessage(f"Recording to {Path(path).name}...")
        elif path:
            self.statusBar().showMessage(f"Saved video {path}", 8000)
            self._update_next_name()

    def _request_timelapse(self, start: bool):
        link = self._link()
        if link is None or link.worker is None:
            return
        if not start:
            link.worker.stop_timelapse()
            return
        opts = {
            "n_frames": self.average_combo.currentData(),
            "scale": self.size_combo.currentData(),
            "fmt": self.format_combo.currentData(),
            "folder": Path(self.folder_edit.text()),
            "name": clean_name(self.name_edit.text()),
        }
        link.timelapse = True
        link.worker.start_timelapse(self.record_panel.make_timelapse(), opts)
        self.record_panel.set_timelapse_running(True)

    def _on_timelapse_progress(self, link: CameraLink, taken: int, total: int, next_due: float, path: str):
        if link is self._link():
            self.record_panel.set_timelapse_progress(taken, total, next_due)
        self.last_saved_label.setText(f"Saved {Path(path).name} (time-lapse)")
        self._update_next_name()

    def _on_timelapse_finished(self, link: CameraLink, message: str):
        if link is self._link():
            self.record_panel.set_timelapse_message(message)
        self.statusBar().showMessage(message, 10000)
        self._update_next_name()

    def closeEvent(self, event):
        busy = [f"on {camera_label(lk.key)}, {' and '.join(self._busy(lk))}" for lk in self._links() if self._busy(lk)]
        if busy and QMessageBox.question(
            self, "Quit", f"{'; '.join(busy).capitalize()}. Stop and quit?"
        ) != QMessageBox.StandardButton.Yes:
            event.ignore()
            return
        if self._job is not None:
            self._job.wait()  # let a running processing job finish first
        self._fiber_waiting.clear()
        if self._fiber_job is not None:
            self._fiber_job.wait()
        self._save_settings()
        for link in self._links():
            link.stop()
        super().closeEvent(event)
