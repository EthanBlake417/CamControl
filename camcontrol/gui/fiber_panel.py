"""Fiber axis panel: finds the slow/fast axes of PM fiber end faces (Panda,
Bow-tie, Elliptical-clad) and draws them on the image.

The measuring is in camcontrol/processing/fiber_axis.py. The panel holds the
results; the image view draws them; the main window runs the analysis on
the image in the view and exports.
"""

import math

from PySide6.QtCore import QPointF, Qt, QTimer, Signal
from PySide6.QtGui import QColor, QImage, QPainter, QPen, QPolygonF
from PySide6.QtWidgets import (
    QAbstractItemView,
    QCheckBox,
    QComboBox,
    QFormLayout,
    QGroupBox,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QPushButton,
    QSlider,
    QTableWidget,
    QTableWidgetItem,
    QVBoxLayout,
    QWidget,
)

from camcontrol.gui.measure_draw import Style, draw_label
from camcontrol.gui.qt_image import to_qimage
from camcontrol.processing.fiber_axis import LOOK_AT, METHODS, Detection, FiberAxis

SLOW_COLOR = QColor(255, 0, 200)   # magenta: shows on blue, white and black
FAST_COLOR = QColor(255, 200, 0)
LINE_LENGTH = 1.25                 # axis lines reach this many radii from the centre

METHOD_TIPS = {
    "symmetry": "The line the end face is most mirror-symmetric about (and at 90° to it).\n"
                "No threshold; lopsided stress parts don't pull it.",
    "centres": "The line through the centres of the two stress parts (Panda, Bow-tie).\n"
               "Each part counts the same whatever its shape. Elliptical fibers use Second moments.",
    "moments": "The principal axis of all stress-part pixels about the fiber centre.\n"
               "Far pixels count most, so the outer corners of bow-tie wedges decide it.",
}

THRESHOLD_STEP = 0.5   # threshold slider: noise units per step
SMOOTHING_STEP = 0.25  # smoothing slider: % of the radius per step
TUNE_DELAY_MS = 150    # re-measure this long after a slider stops moving

LOOK_AT_TIPS = {
    "both": "Stress parts differ from the cladding in brightness, color or both.",
    "brightness": "Only brightness counts. For gray stress parts, or when color fringes\n"
                  "(e.g. moiré, dirt, colored light) get mistaken for stress parts.",
    "color": "Only color counts. For colored stress parts under uneven brightness.",
}

HINT = ("Angles: 0° is horizontal, + is turned counter-clockwise, - clockwise (-90° to +90°). Solid magenta line: slow axis (through the "
        "stress rods / wedges / ellipse). Dashed yellow: fast axis. Fibers are found automatically; "
        "for one that isn't, draw a Circle (3 pt or centre) round it in Measurements, then Find axes.")


def signed_angle(deg: float) -> float:
    """An axis angle as -90 to +90 degrees: 0 is horizontal, + is turned
    counter-clockwise, - clockwise. Easier to read when lining fibers up."""
    a = (deg + 90) % 180 - 90
    return 90.0 if a == -90 else a


def fmt_angle(deg: float) -> str:
    a = round(signed_angle(deg), 1)
    return f"{0.0 if a == 0 else a:+.1f}"  # no "-0.0"


def draw_fiber_axes(painter: QPainter, axes: list[FiberAxis], to_out, zoom: float, style: Style,
                    selected: int | None = None, show_parts: bool = True):
    """Each fiber's outline, slow axis (solid) and fast axis (dashed), with a label.
    With show_parts, also the stress parts found (shaded, a cross at each one's
    centre) and the point the axis turns about (a small ring)."""
    for i, f in enumerate(axes):
        centre = to_out(QPointF(f.cx, f.cy))
        pivot = to_out(QPointF(*f.axis_point))
        r = f.radius * zoom
        width = style.line_width * (1.6 if i == selected else 1)
        if show_parts:
            draw_stress_parts(painter, f, to_out, width, style)
        painter.setBrush(Qt.BrushStyle.NoBrush)
        painter.setPen(QPen(QColor(255, 255, 255, 160), max(1.0, width / 2), Qt.PenStyle.DotLine))
        painter.drawEllipse(centre, r, r)
        for deg, color, dash in ((f.fast_deg, FAST_COLOR, Qt.PenStyle.DashLine),
                                 (f.slow_deg, SLOW_COLOR, Qt.PenStyle.SolidLine)):
            a = math.radians(deg)
            d = QPointF(math.cos(a), -math.sin(a)) * (r * LINE_LENGTH)  # screen y points down
            painter.setPen(QPen(color, width, dash))
            painter.drawLine(pivot - d, pivot + d)
        if show_parts:  # the point the axis turns about
            painter.setBrush(Qt.BrushStyle.NoBrush)
            painter.setPen(QPen(QColor(255, 255, 255), max(1.0, width / 2)))
            painter.drawEllipse(pivot, style.marker * 1.5, style.marker * 1.5)
        # Label above the fiber (draw_label puts the text below-right of the point).
        text = f"{i + 1}: {fmt_angle(f.slow_deg)}°" + ("" if f.clear else " (unclear)")
        above = centre + QPointF(-r * 0.7, -r * LINE_LENGTH - style.font_px * 2.2)
        draw_label(painter, above, text, SLOW_COLOR, style)


def draw_stress_parts(painter: QPainter, f: FiberAxis, to_out, width: float, style: Style):
    """The stress parts the axis was measured from: shaded outlines and a cross at each centre."""
    fill = QColor(SLOW_COLOR)
    fill.setAlpha(45)
    painter.setBrush(fill)
    painter.setPen(QPen(SLOW_COLOR, max(1.0, width / 2), Qt.PenStyle.DotLine))
    for outline in f.parts:
        painter.drawPolygon(QPolygonF([to_out(QPointF(x, y)) for x, y in outline]))
    painter.setPen(QPen(SLOW_COLOR, width))
    k = style.marker * 1.5
    for x, y in f.part_centres:
        c = to_out(QPointF(x, y))
        painter.drawLine(c + QPointF(-k, -k), c + QPointF(k, k))
        painter.drawLine(c + QPointF(-k, k), c + QPointF(k, -k))


def render_fiber_axes(image, axes: list[FiberAxis], show_parts: bool = True) -> QImage:
    """A full-resolution copy of the image with the axes drawn on it."""
    qimg = to_qimage(image).convertToFormat(QImage.Format.Format_RGB32)
    w = image.shape[1]
    style = Style(line_width=max(2.0, w / 600), font_px=max(14, round(w / 60)))
    painter = QPainter(qimg)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    draw_fiber_axes(painter, axes, lambda p: p, 1.0, style, show_parts=show_parts)
    painter.end()
    return qimg


class FiberPanel(QWidget):
    find_requested = Signal()
    method_changed = Signal()       # method or "about stress parts" changed
    live_changed = Signal(bool)     # "Update live" toggled
    changed = Signal()              # results changed: redraw
    export_requested = Signal()

    def __init__(self, parent=None):
        super().__init__(parent)
        self.axes: list[FiberAxis] = []
        layout = QHBoxLayout(self)

        left = QVBoxLayout()
        find = QPushButton("Find axes")
        find.setToolTip("Measure the fibers in the image in the view.")
        find.clicked.connect(self.find_requested)
        left.addWidget(find)
        self.live_check = QCheckBox("Update live")
        self.live_check.setToolTip("Keep measuring the live view (a few times a second),\n"
                                   "e.g. while rotating a fiber to line up its axis.")
        self.live_check.toggled.connect(self.live_changed)
        left.addWidget(self.live_check)
        self.method_combo = QComboBox()
        for i, (name, label) in enumerate(METHODS.items()):
            self.method_combo.addItem(label, name)
            self.method_combo.setItemData(i, METHOD_TIPS[name], Qt.ItemDataRole.ToolTipRole)
        self.method_combo.setToolTip("How the axis angle is worked out. Hover over a choice for details.")
        self.method_combo.currentIndexChanged.connect(self.method_changed)
        method_row = QHBoxLayout()
        method_row.addWidget(QLabel("Method:"))
        method_row.addWidget(self.method_combo, stretch=1)
        left.addLayout(method_row)
        self.parts_centre_check = QCheckBox("Turn about stress parts")
        self.parts_centre_check.setToolTip(
            "Measure the axis about the centre of the stress parts instead of the fiber centre\n"
            "(Mirror symmetry and Second moments; for stress parts off the fiber's middle).")
        self.parts_centre_check.toggled.connect(self.method_changed)
        left.addWidget(self.parts_centre_check)
        self.show_parts_check = QCheckBox("Show stress parts")
        self.show_parts_check.setChecked(True)
        self.show_parts_check.setToolTip(
            "Shade the stress parts the axis was measured from, with a cross at each one's centre,\n"
            "and ring the point the axis turns about. Also used for the exported drawing.")
        left.addWidget(self.show_parts_check)
        self.use_circles = QCheckBox("Use circle measurements")
        self.use_circles.setChecked(True)
        self.use_circles.setToolTip("If there are Circle measurements, measure the fibers inside them\n"
                                    "instead of finding fibers automatically.")
        left.addWidget(self.use_circles)
        clear = QPushButton("Clear")
        clear.clicked.connect(lambda: self.set_axes([]))
        export = QPushButton("Export...")
        export.setToolTip("Save the table as Excel or CSV, plus the image with the axes drawn on.")
        export.clicked.connect(self.export_requested)
        left.addWidget(clear)
        left.addWidget(export)
        left.addStretch()
        layout.addLayout(left)
        layout.addWidget(self._build_detection())

        right = QVBoxLayout()
        self.table = QTableWidget(0, 9)
        self.table.setHorizontalHeaderLabels(["#", "Type", "Slow axis (°)", "Fast axis (°)", "Clarity", "Method",
                                              "Centre (px)", "Axis through (px)", "Diameter (px)"])
        self.table.setSelectionBehavior(QAbstractItemView.SelectionBehavior.SelectRows)
        self.table.setSelectionMode(QAbstractItemView.SelectionMode.SingleSelection)
        self.table.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers)
        self.table.verticalHeader().setVisible(False)
        self.table.horizontalHeader().setSectionResizeMode(QHeaderView.ResizeMode.ResizeToContents)
        self.table.horizontalHeader().setStretchLastSection(True)
        self.table.itemSelectionChanged.connect(self.changed)
        self.table.horizontalHeaderItem(4).setToolTip(
            "How much longer the stress parts spread along the slow axis than across it.\n"
            "Below 1.3 the axis isn't clear (e.g. a plain round fiber, or poor contrast).")
        right.addWidget(self.table)
        hint = QLabel(HINT)
        hint.setWordWrap(True)
        hint.setStyleSheet("color: gray;")
        right.addWidget(hint)
        layout.addLayout(right, stretch=1)

    def _build_detection(self) -> QGroupBox:
        """Controls for telling stress parts from the cladding, for low-contrast images."""
        box = QGroupBox("Stress part detection")
        box.setToolTip("Tune these when the shaded stress parts don't match what you see\n"
                       "(turn on Show stress parts and Update live to watch them change).")
        form = QFormLayout(box)
        self._tune_timer = QTimer(self, singleShot=True, interval=TUNE_DELAY_MS)
        self._tune_timer.timeout.connect(self.method_changed)

        self.look_combo = QComboBox()
        for i, (name, (label, _)) in enumerate(LOOK_AT.items()):
            self.look_combo.addItem(label, name)
            self.look_combo.setItemData(i, LOOK_AT_TIPS[name], Qt.ItemDataRole.ToolTipRole)
        self.look_combo.setToolTip("What makes a stress part different from the cladding. Hover over a choice for details.")
        self.look_combo.currentIndexChanged.connect(self.method_changed)
        form.addRow("Look at:", self.look_combo)

        self.auto_check = QCheckBox("Auto")
        self.auto_check.setChecked(True)
        self.auto_check.setToolTip("Pick the threshold automatically (Otsu's method).\n"
                                   "Untick to set it yourself, starting from the automatic value.")
        self.auto_check.toggled.connect(self._auto_toggled)
        self.threshold_slider, self.threshold_label = self._slider(1, 80, 12, lambda v: f"{v * THRESHOLD_STEP:.1f}")
        self.threshold_slider.setToolTip(
            "How different from the cladding a pixel must be to count as stress part,\n"
            "in multiples of the cladding's own noise. Lower finds fainter parts, but also noise.")
        self.threshold_slider.setEnabled(False)
        row = QHBoxLayout()
        row.addWidget(self.auto_check)
        row.addWidget(self.threshold_slider, stretch=1)
        row.addWidget(self.threshold_label)
        form.addRow("Threshold:", row)

        self.smoothing_slider, label = self._slider(0, 20, 4, lambda v: f"{v * SMOOTHING_STEP:.2f}%")
        self.smoothing_slider.setToolTip("Blur before comparing, as % of the fiber radius.\n"
                                         "More smooths out noise and fine texture, but rounds off small parts.")
        form.addRow("Smoothing:", self._row(self.smoothing_slider, label))

        self.min_size_slider, label = self._slider(1, 40, 8, lambda v: f"{v}%")
        self.min_size_slider.setToolTip("Leave out regions smaller than a square this wide (% of the fiber radius):\n"
                                        "dust, scratches, noise.")
        form.addRow("Ignore specks:", self._row(self.min_size_slider, label))

        defaults = QPushButton("Defaults")
        defaults.setToolTip("Back to the standard detection settings.")
        defaults.clicked.connect(lambda: self.set_detection(Detection()))
        form.addRow(defaults)
        return box

    def _slider(self, low: int, high: int, value: int, text) -> tuple[QSlider, QLabel]:
        slider = QSlider(Qt.Orientation.Horizontal)
        slider.setRange(low, high)
        slider.setValue(value)
        slider.setMinimumWidth(100)
        label = QLabel(text(value))
        label.setMinimumWidth(label.fontMetrics().horizontalAdvance("00.00%"))
        slider.valueChanged.connect(lambda v: label.setText(text(v)))
        slider.valueChanged.connect(lambda: self._tune_timer.start())
        return slider, label

    @staticmethod
    def _row(slider: QSlider, label: QLabel) -> QHBoxLayout:
        row = QHBoxLayout()
        row.addWidget(slider, stretch=1)
        row.addWidget(label)
        return row

    def _auto_toggled(self, auto: bool):
        self.threshold_slider.setEnabled(not auto)
        self.method_changed.emit()

    def detection(self) -> Detection:
        return Detection(
            threshold=None if self.auto_check.isChecked() else self.threshold_slider.value() * THRESHOLD_STEP,
            smoothing=self.smoothing_slider.value() * SMOOTHING_STEP,
            look_at=self.look_combo.currentData(),
            min_size=float(self.min_size_slider.value()))

    def set_detection(self, d: Detection):
        """Show d's settings (one re-measure, not one per control)."""
        widgets = (self.look_combo, self.auto_check, self.threshold_slider, self.smoothing_slider, self.min_size_slider)
        for w in widgets:
            w.blockSignals(True)
        i = self.look_combo.findData(d.look_at)
        self.look_combo.setCurrentIndex(max(i, 0))
        self.auto_check.setChecked(d.threshold is None)
        self.threshold_slider.setEnabled(d.threshold is not None)
        if d.threshold is not None:
            self._show_threshold(d.threshold)
        self.smoothing_slider.setValue(round(d.smoothing / SMOOTHING_STEP))
        self.min_size_slider.setValue(round(d.min_size))
        for w in widgets:
            w.blockSignals(False)
        self.smoothing_slider.valueChanged.emit(self.smoothing_slider.value())  # update the labels
        self.min_size_slider.valueChanged.emit(self.min_size_slider.value())
        self._tune_timer.stop()  # started by those emits
        self.method_changed.emit()

    def _show_threshold(self, value: float):
        """Put the slider at value without re-measuring."""
        self.threshold_slider.blockSignals(True)
        self.threshold_slider.setValue(round(value / THRESHOLD_STEP))
        self.threshold_slider.blockSignals(False)
        self.threshold_label.setText(f"{self.threshold_slider.value() * THRESHOLD_STEP:.1f}")

    def method(self) -> str:
        return self.method_combo.currentData()

    def set_method(self, name: str):
        i = self.method_combo.findData(name)
        if i >= 0:
            self.method_combo.setCurrentIndex(i)

    def selected(self) -> int | None:
        rows = self.table.selectionModel().selectedRows()
        return rows[0].row() if rows else None

    def set_axes(self, axes: list[FiberAxis]):
        # Left to right, then top to bottom, so numbers stay put between live updates.
        self.axes = sorted(axes, key=lambda f: (round(f.cy / max(f.radius, 1)), f.cx))
        keep = self.selected()
        self.table.blockSignals(True)
        self.table.setRowCount(len(self.axes))
        for row, f in enumerate(self.axes):
            values = [str(row + 1), f.fiber_type, fmt_angle(f.slow_deg), fmt_angle(f.fast_deg),
                      f"{f.elongation:.2f}" + ("" if f.clear else " (unclear)"), METHODS[f.method],
                      f"{f.cx:.1f}, {f.cy:.1f}", "{:.1f}, {:.1f}".format(*f.axis_point), f"{2 * f.radius:.1f}"]
            for col, text in enumerate(values):
                item = QTableWidgetItem(text)
                if col in (2, 3, 4, 8):
                    item.setTextAlignment(Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter)
                self.table.setItem(row, col, item)
        if keep is not None and keep < len(self.axes):
            self.table.selectRow(keep)
        self.table.blockSignals(False)
        if self.auto_check.isChecked() and self.axes:  # show the automatic threshold, to start from
            self._show_threshold(self.axes[keep if keep is not None and keep < len(self.axes) else 0].threshold)
        self.changed.emit()

    def rows(self, source: str) -> list[dict]:
        return [{"#": i + 1, "Type": f.fiber_type, "Slow axis (deg)": round(signed_angle(f.slow_deg), 2),
                 "Fast axis (deg)": round(signed_angle(f.fast_deg), 2), "Clarity": round(f.elongation, 3),
                 "Method": METHODS[f.method], "Threshold (noise x)": round(f.threshold, 2),
                 "Centre x (px)": round(f.cx, 1), "Centre y (px)": round(f.cy, 1),
                 "Axis x (px)": round(f.axis_point[0], 1), "Axis y (px)": round(f.axis_point[1], 1),
                 "Diameter (px)": round(2 * f.radius, 1), "Image": source}
                for i, f in enumerate(self.axes)]
