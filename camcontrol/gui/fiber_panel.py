"""Fiber axis panel: finds the slow/fast axes of PM fiber end faces (Panda,
Bow-tie, Elliptical-clad) and draws them on the image.

The measuring is in camcontrol/processing/fiber_axis.py. The panel holds the
results; the image view draws them; the main window runs the analysis on
the image in the view and exports.
"""

import math

from PySide6.QtCore import QPointF, Qt, Signal
from PySide6.QtGui import QColor, QImage, QPainter, QPen
from PySide6.QtWidgets import (
    QAbstractItemView,
    QCheckBox,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QPushButton,
    QTableWidget,
    QTableWidgetItem,
    QVBoxLayout,
    QWidget,
)

from camcontrol.gui.measure_draw import Style, draw_label
from camcontrol.gui.qt_image import to_qimage
from camcontrol.processing.fiber_axis import FiberAxis

SLOW_COLOR = QColor(255, 0, 200)   # magenta: shows on blue, white and black
FAST_COLOR = QColor(255, 200, 0)
LINE_LENGTH = 1.25                 # axis lines reach this many radii from the centre

HINT = ("Angles: 0° is horizontal, + is turned counter-clockwise, - clockwise (-90° to +90°). Solid magenta line: slow axis (through the "
        "stress rods / wedges / ellipse). Dashed yellow: fast axis. Fibers are found automatically; "
        "for one that isn't, draw a Circle (3 pt) round it in Measurements, then Find axes.")


def signed_angle(deg: float) -> float:
    """An axis angle as -90 to +90 degrees: 0 is horizontal, + is turned
    counter-clockwise, - clockwise. Easier to read when lining fibers up."""
    a = (deg + 90) % 180 - 90
    return 90.0 if a == -90 else a


def fmt_angle(deg: float) -> str:
    a = round(signed_angle(deg), 1)
    return f"{0.0 if a == 0 else a:+.1f}"  # no "-0.0"


def draw_fiber_axes(painter: QPainter, axes: list[FiberAxis], to_out, zoom: float, style: Style,
                    selected: int | None = None):
    """Each fiber's outline, slow axis (solid) and fast axis (dashed), with a label."""
    for i, f in enumerate(axes):
        centre = to_out(QPointF(f.cx, f.cy))
        r = f.radius * zoom
        width = style.line_width * (1.6 if i == selected else 1)
        painter.setBrush(Qt.BrushStyle.NoBrush)
        painter.setPen(QPen(QColor(255, 255, 255, 160), max(1.0, width / 2), Qt.PenStyle.DotLine))
        painter.drawEllipse(centre, r, r)
        for deg, color, dash in ((f.fast_deg, FAST_COLOR, Qt.PenStyle.DashLine),
                                 (f.slow_deg, SLOW_COLOR, Qt.PenStyle.SolidLine)):
            a = math.radians(deg)
            d = QPointF(math.cos(a), -math.sin(a)) * (r * LINE_LENGTH)  # screen y points down
            painter.setPen(QPen(color, width, dash))
            painter.drawLine(centre - d, centre + d)
        # Label above the fiber (draw_label puts the text below-right of the point).
        text = f"{i + 1}: {fmt_angle(f.slow_deg)}°" + ("" if f.clear else " (unclear)")
        above = centre + QPointF(-r * 0.7, -r * LINE_LENGTH - style.font_px * 2.2)
        draw_label(painter, above, text, SLOW_COLOR, style)


def render_fiber_axes(image, axes: list[FiberAxis]) -> QImage:
    """A full-resolution copy of the image with the axes drawn on it."""
    qimg = to_qimage(image).convertToFormat(QImage.Format.Format_RGB32)
    w = image.shape[1]
    style = Style(line_width=max(2.0, w / 600), font_px=max(14, round(w / 60)))
    painter = QPainter(qimg)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    draw_fiber_axes(painter, axes, lambda p: p, 1.0, style)
    painter.end()
    return qimg


class FiberPanel(QWidget):
    find_requested = Signal()
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

        right = QVBoxLayout()
        self.table = QTableWidget(0, 7)
        self.table.setHorizontalHeaderLabels(
            ["#", "Type", "Slow axis (°)", "Fast axis (°)", "Clarity", "Centre (px)", "Diameter (px)"])
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
                      f"{f.elongation:.2f}" + ("" if f.clear else " (unclear)"),
                      f"{f.cx:.1f}, {f.cy:.1f}", f"{2 * f.radius:.1f}"]
            for col, text in enumerate(values):
                item = QTableWidgetItem(text)
                if col in (2, 3, 4, 6):
                    item.setTextAlignment(Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter)
                self.table.setItem(row, col, item)
        if keep is not None and keep < len(self.axes):
            self.table.selectRow(keep)
        self.table.blockSignals(False)
        self.changed.emit()

    def rows(self, source: str) -> list[dict]:
        return [{"#": i + 1, "Type": f.fiber_type, "Slow axis (deg)": round(signed_angle(f.slow_deg), 2),
                 "Fast axis (deg)": round(signed_angle(f.fast_deg), 2), "Clarity": round(f.elongation, 3),
                 "Centre x (px)": round(f.cx, 1), "Centre y (px)": round(f.cy, 1),
                 "Diameter (px)": round(2 * f.radius, 1), "Image": source}
                for i, f in enumerate(self.axes)]
