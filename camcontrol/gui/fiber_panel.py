"""Fiber axis panel: finds the slow/fast axes of PM fiber end faces (Panda,
Bow-tie, Elliptical-clad) and draws them on the image.

The measuring is in camcontrol/processing/fiber_axis.py. The panel holds the
results; the image view draws them (with the angle readout in its top-left
corner); the main window runs the analysis on the image in the view and
exports (Measure menu).
"""

import math
from dataclasses import dataclass, field

from PySide6.QtCore import QPointF, QRectF, Qt, QTimer, Signal
from PySide6.QtGui import QColor, QFont, QFontMetricsF, QImage, QPainter, QPen, QPolygonF
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
    QSpinBox,
    QTableWidget,
    QTableWidgetItem,
    QVBoxLayout,
    QWidget,
)

from camcontrol.gui.measure_draw import Style
from camcontrol.gui.overlay_settings import FiberLineStyle
from camcontrol.gui.qt_image import to_qimage
from camcontrol.processing.fiber_axis import LOOK_AT, METHODS, Detection, FiberAxis, brightest_centre

SLOW_COLOR = QColor(255, 0, 200)   # magenta: shows on blue, white and black
FAST_COLOR = QColor(255, 200, 0)
READOUT_COLOR = QColor(0, 255, 0)
LINE_LENGTH = 1.25                 # axis lines reach this many radii from the centre
READOUT_PX = 26                    # readout text height on screen; exports scale it with the image

METHOD_TIPS = {
    "auto": "Works out the fiber type and uses the best method for it: Panda rod outlines\n"
            "for Panda fibers, Mirror symmetry for Bow-tie and Elliptical ones; the Apple core\n"
            "when the rods are washed out (or to confirm them).",
    "rods": "For Panda fibers: finds the round outlines of the two stress rods and takes\n"
            "the line through their centres. Uses only edges, so shading and dark patches\n"
            "don't matter, and needs no detection settings. Unclear if the outlines are faint\n"
            "or not round (e.g. Bow-tie fibers: use Mirror symmetry for those).",
    "core": "For Panda fibers: the bright \"apple core\" round the fiber's core, pinched on the\n"
            "two sides where the rods press on it. Its mirror axes are the fiber's axes; the one\n"
            "through the pinches is the slow axis. Works when the rods are washed out (grey or\n"
            "front-lit pictures). Unclear if the core isn't symmetric or isn't pinched.",
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

HINT = ("Clarity: how sure the result is; \"(unclear)\" means check it by eye. "
        "Angles: 0° is horizontal, + is turned counter-clockwise, - clockwise (-90° to +90°). Solid magenta line: slow axis (through the "
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


def clarity_text(f: FiberAxis) -> str:
    if f.coverage is not None:  # "rods": how much of the rod outlines was found
        return f"{f.coverage:.0%} outline"
    return f"{f.elongation:.2f}"


def sort_axes(axes: list[FiberAxis]) -> list[FiberAxis]:
    """Left to right, then top to bottom, so numbers stay put between live updates."""
    return sorted(axes, key=lambda f: (round(f.cy / max(f.radius, 1)), f.cx))


@dataclass
class FiberDrawing:
    """How fiber axes are drawn (on screen, on exported copies and on videos)."""
    show_parts: bool = False
    # Axis lines only outside the fibers, from each one's edge to the area's edges, to
    # compare with the crosshair, grid or the image's own lines without covering the fiber.
    long_lines: bool = False
    slow: QColor = field(default_factory=lambda: QColor(SLOW_COLOR))  # line colors (with opacity);
    fast: QColor = field(default_factory=lambda: QColor(FAST_COLOR))  # the readout's angles too
    show_slow: bool = True
    show_fast: bool = True
    # Colors by angle (overlay_settings.AngleColors.bands): aim, [(limit, color)], smallest first.
    slow_bands: tuple | None = None
    fast_bands: tuple | None = None

    @classmethod
    def from_style(cls, style: FiberLineStyle, show_parts: bool, long_lines: bool) -> "FiberDrawing":
        return cls(show_parts, long_lines, style.color(True), style.color(False),
                   style.show_axes in ("slow", "both"), style.show_axes in ("fast", "both"),
                   style.slow_by_angle.bands(style.slow_opacity), style.fast_by_angle.bands(style.fast_opacity))

    @staticmethod
    def _by_angle(base: QColor, bands, deg: float) -> QColor:
        if bands is None:
            return base
        aim, limits = bands
        off = abs(abs(signed_angle(deg)) - aim)  # from the aim, either way
        return next((c for limit, c in limits if off <= limit), base)

    def slow_color(self, f: FiberAxis) -> QColor:
        """This fiber's slow axis color (its own, or by its angle)."""
        return self._by_angle(self.slow, self.slow_bands, f.slow_deg)

    def fast_color(self, f: FiberAxis) -> QColor:
        return self._by_angle(self.fast, self.fast_bands, f.fast_deg)


def outside_circles(a: QPointF, b: QPointF, circles: list[tuple[QPointF, float]]) -> list[tuple[QPointF, QPointF]]:
    """The parts of the line a..b outside every circle (centre, radius)."""
    d = b - a
    len2 = d.x() ** 2 + d.y() ** 2
    if len2 <= 0:
        return []
    covered = []  # where along a..b (0 to 1) each circle covers it
    for c, r in circles:
        o = a - c
        half_b = (o.x() * d.x() + o.y() * d.y()) / len2
        disc = half_b * half_b - (o.x() ** 2 + o.y() ** 2 - r * r) / len2
        if disc > 0:
            covered.append((-half_b - math.sqrt(disc), -half_b + math.sqrt(disc)))
    pieces, start = [], 0.0
    for t0, t1 in sorted(covered):
        if t0 > start:
            pieces.append((a + d * start, a + d * min(t0, 1.0)))
        start = max(start, t1)
        if start >= 1:
            break
    if start < 1:
        pieces.append((a + d * start, b))
    return [(p, q) for p, q in pieces if math.hypot(q.x() - p.x(), q.y() - p.y()) > 0.5]


def clip_to_rect(a: QPointF, b: QPointF, rect: QRectF) -> tuple[QPointF, QPointF] | None:
    """The part of the line a..b inside rect (Liang-Barsky), if any."""
    d = b - a
    t0, t1 = 0.0, 1.0
    for p, q in ((-d.x(), a.x() - rect.left()), (d.x(), rect.right() - a.x()),
                 (-d.y(), a.y() - rect.top()), (d.y(), rect.bottom() - a.y())):
        if p == 0:
            if q < 0:
                return None
        elif p < 0:
            t0 = max(t0, q / p)
        else:
            t1 = min(t1, q / p)
    return (a + d * t0, a + d * t1) if t0 < t1 else None


def draw_fiber_axes(painter: QPainter, axes: list[FiberAxis], to_out, zoom: float, style: Style,
                    selected: int | None = None, drawing: FiberDrawing | None = None, area: QRectF | None = None):
    """Each fiber's outline, slow axis (solid) and fast axis (dashed). With
    drawing.show_parts, also the stress parts found (shaded, a cross at each
    one's centre) and the point the axis turns about (a small ring). The fiber
    with the brightest centre (of two or more) is ringed in green. area: where
    long lines end (the view on screen, the picture on exports). No number tag
    on the fibers: the readout numbers them left to right (draw_fiber_readout)."""
    drawing = drawing or FiberDrawing()
    brightest = brightest_centre(axes)
    circles = [(to_out(QPointF(f.cx, f.cy)), f.radius * zoom) for f in axes]
    for i, f in enumerate(axes):
        centre = to_out(QPointF(f.cx, f.cy))
        pivot = to_out(QPointF(*f.axis_point))
        r = f.radius * zoom
        width = style.line_width * (1.6 if i == selected else 1)
        if drawing.show_parts:
            draw_stress_parts(painter, f, to_out, width, style, drawing.slow)
        painter.setBrush(Qt.BrushStyle.NoBrush)
        painter.setPen(QPen(QColor(255, 255, 255, 160), max(1.0, width / 2), Qt.PenStyle.DotLine))
        painter.drawEllipse(centre, r, r)
        if brightest is not None and brightest[0] == i:
            k = max(r * 0.25, style.marker * 3)
            painter.setPen(QPen(READOUT_COLOR, width))
            painter.drawEllipse(centre, k, k)
        shown = [(drawing.show_fast, f.fast_deg, drawing.fast_color(f), Qt.PenStyle.DashLine),
                 (drawing.show_slow, f.slow_deg, drawing.slow_color(f), Qt.PenStyle.SolidLine)]
        for show, deg, color, dash in shown:
            if not show:
                continue
            a = math.radians(deg)
            painter.setPen(QPen(color, width, dash))
            if drawing.long_lines and area is not None:
                # Through the pivot to the edges of the area (as the crosshair), never over a
                # fiber (this one or its neighbours); still drawn when the pivot is out of view.
                far = area.width() + area.height() + abs(pivot.x() - area.left()) + abs(pivot.y() - area.top())
                u = QPointF(math.cos(a), -math.sin(a)) * far  # screen y points down
                clipped = clip_to_rect(pivot - u, pivot + u, area)
                if clipped is not None:
                    for p, q in outside_circles(*clipped, circles):
                        painter.drawLine(p, q)
            else:
                d = QPointF(math.cos(a), -math.sin(a)) * (r * LINE_LENGTH)
                painter.drawLine(pivot - d, pivot + d)
        if drawing.show_parts:  # the point the axis turns about
            painter.setBrush(Qt.BrushStyle.NoBrush)
            painter.setPen(QPen(QColor(255, 255, 255), max(1.0, width / 2)))
            painter.drawEllipse(pivot, style.marker * 1.5, style.marker * 1.5)


def draw_stress_parts(painter: QPainter, f: FiberAxis, to_out, width: float, style: Style, color: QColor):
    """The stress parts the axis was measured from: shaded outlines and a cross at each centre."""
    fill = QColor(color)
    fill.setAlpha(45)
    painter.setBrush(fill)
    painter.setPen(QPen(color, max(1.0, width / 2), Qt.PenStyle.DotLine))
    for outline in f.parts:
        painter.drawPolygon(QPolygonF([to_out(QPointF(x, y)) for x, y in outline]))
    painter.setPen(QPen(color, width))
    k = style.marker * 1.5
    for x, y in f.part_centres:
        c = to_out(QPointF(x, y))
        painter.drawLine(c + QPointF(-k, -k), c + QPointF(k, k))
        painter.drawLine(c + QPointF(-k, k), c + QPointF(k, -k))


def fiber_readout_pieces(axes: list[FiberAxis], drawing: FiberDrawing | None = None) -> list[list[tuple[str, QColor]]]:
    """The readout's lines as colored pieces: per fiber (numbered left to right) the
    slow and fast axis angles in their line colors (opaque, to stay readable), the
    number in the first one's, "(unclear)" when unsure; with several fibers, a last
    line saying which has the brightest centre."""
    drawing = drawing or FiberDrawing()

    def opaque(c: QColor) -> QColor:
        c = QColor(c)
        c.setAlpha(255)
        return c
    lines = []
    for i, f in enumerate(axes):
        line = []
        if drawing.show_slow:
            line.append((f"slow {fmt_angle(f.slow_deg)}°", opaque(drawing.slow_color(f))))
        if drawing.show_fast:
            line.append((("  " if line else "") + f"fast {fmt_angle(f.fast_deg)}°", opaque(drawing.fast_color(f))))
        line.insert(0, (f"{i + 1}: ", line[0][1] if line else READOUT_COLOR))
        if not f.clear:
            line.append((" (unclear)", QColor(230, 230, 230)))
        lines.append(line)
    brightest = brightest_centre(axes)
    if brightest is not None:
        i, position = brightest
        lines.append([(f"Brightest centre: position {position} from right (fiber {i + 1})", READOUT_COLOR)])
    return lines


def fiber_readout_lines(axes: list[FiberAxis]) -> list[str]:
    """The readout's lines as plain text."""
    return ["".join(text for text, _ in line) for line in fiber_readout_pieces(axes)]


def draw_fiber_readout(painter: QPainter, axes: list[FiberAxis], drawing: FiberDrawing | None,
                       corner: QPointF, font_px: float):
    """The angles in a dark box with its top-left corner at corner."""
    lines = fiber_readout_pieces(axes, drawing)
    if not lines:
        return
    font = QFont(painter.font())
    font.setPixelSize(max(1, round(font_px)))
    painter.setFont(font)
    metrics = QFontMetricsF(font)
    pad, step = font_px * 0.4, font_px * 1.3
    widths = [[metrics.horizontalAdvance(text) for text, _ in line] for line in lines]
    w = max(sum(ws) for ws in widths) + 2 * pad
    h = step * len(lines) + 2 * pad - (step - font_px * 1.15)
    painter.fillRect(QRectF(corner.x(), corner.y(), w, h), QColor(0, 0, 0, 170))
    for k, (line, ws) in enumerate(zip(lines, widths)):
        x = corner.x() + pad
        for (text, color), tw in zip(line, ws):
            painter.setPen(color)
            painter.drawText(QRectF(x, corner.y() + pad + k * step, tw + font_px, step),
                             Qt.AlignmentFlag.AlignLeft | Qt.AlignmentFlag.AlignTop, text)
            x += tw


def paint_fiber_axes(qimg: QImage, axes: list[FiberAxis], drawing: FiberDrawing | None = None):
    """Draw the axes and the readout on qimg (any thread: it's a QImage), at the picture's scale."""
    w, h = qimg.width(), qimg.height()
    style = Style(line_width=max(2.0, w / 600), font_px=max(14, round(w / 60)))
    painter = QPainter(qimg)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    draw_fiber_axes(painter, axes, lambda p: p, 1.0, style, drawing=drawing, area=QRectF(0, 0, w, h))
    # Big, as on screen: READOUT_PX on a view about 1000 px wide.
    font = max(18, round(w / 1000 * READOUT_PX))
    draw_fiber_readout(painter, axes, drawing, QPointF(font * 0.4, font * 0.4), font)
    painter.end()


def render_fiber_axes(image, axes: list[FiberAxis], drawing: FiberDrawing | None = None) -> QImage:
    """A full-resolution copy of the image with the axes and the angle readout drawn on it."""
    qimg = to_qimage(image).convertToFormat(QImage.Format.Format_RGB32)
    paint_fiber_axes(qimg, axes, drawing)
    return qimg


class FiberPanel(QWidget):
    find_requested = Signal()
    method_changed = Signal()       # method or "about stress parts" changed
    live_changed = Signal(bool)     # "Update live" toggled
    changed = Signal()              # results changed: redraw
    draw_rod_requested = Signal(bool)  # Smallest rod > Draw: start (True) / stop drawing it on the view

    def __init__(self, parent=None):
        super().__init__(parent)
        self.axes: list[FiberAxis] = []
        self._min_rod = Detection().min_rod
        layout = QHBoxLayout(self)

        left = QVBoxLayout()
        buttons = QHBoxLayout()
        find = QPushButton("Find axes")
        find.setToolTip("Measure the fibers in the image in the view.")
        find.setMinimumHeight(30)
        find.setStyleSheet("font-weight: bold;")
        find.clicked.connect(self.find_requested)
        clear = QPushButton("Clear")
        clear.setToolTip("Remove the axes from the view.")
        clear.setMinimumHeight(30)
        clear.clicked.connect(lambda: self.set_axes([]))
        buttons.addWidget(find, stretch=2)
        buttons.addWidget(clear, stretch=1)
        left.addLayout(buttons)
        self.live_check = QCheckBox("Update live")
        self.live_check.setToolTip("Keep measuring the live view (a few times a second),\n"
                                   "e.g. while rotating a fiber to line up its axis;\n"
                                   "measure each saved image as it's opened, and the frames of a video\n"
                                   "as it plays (as often as it keeps up) or as you step through it.")
        self.live_check.toggled.connect(self.live_changed)
        left.addWidget(self.live_check)
        self.show_parts_check = QCheckBox("Show stress parts")
        self.show_parts_check.setChecked(True)
        self.show_parts_check.setToolTip(
            "Shade the stress parts the axis was measured from, with a cross at each one's centre,\n"
            "and ring the point the axis turns about. Also used for the exported drawing.")
        left.addWidget(self.show_parts_check)
        self.centre_only_check = QCheckBox("Only the centre fiber")
        self.centre_only_check.setToolTip(
            "Of the fibers found automatically, measure only the one nearest the middle of the image\n"
            "(e.g. the lit fiber in a V-groove, with its neighbours ignored). Faster for Update live.")
        self.centre_only_check.toggled.connect(self.method_changed)
        left.addWidget(self.centre_only_check)
        self.method_combo = QComboBox()
        for i, (name, label) in enumerate(METHODS.items()):
            self.method_combo.addItem(label, name)
            self.method_combo.setItemData(i, METHOD_TIPS[name], Qt.ItemDataRole.ToolTipRole)
        self.method_combo.setToolTip("How the axis angle is worked out. Hover over a choice for details.\n"
                                     "Automatic picks the method for each fiber.")
        self.method_combo.currentIndexChanged.connect(self.method_changed)
        method_row = QHBoxLayout()
        method_row.addWidget(QLabel("Method:"))
        method_row.addWidget(self.method_combo, stretch=1)
        left.addLayout(method_row)
        self.advanced_check = QCheckBox("Show advanced settings")
        self.advanced_check.setToolTip("Smallest rod, turning about the stress parts, circle measurements and the\n"
                                       "stress part detection settings. Not needed normally.")
        self.advanced_check.toggled.connect(self._show_advanced)
        left.addWidget(self.advanced_check)

        self.advanced = QWidget()  # shown with "Show advanced settings"
        advanced = QVBoxLayout(self.advanced)
        advanced.setContentsMargins(0, 0, 0, 0)
        rod_row = QHBoxLayout()
        rod_row.addWidget(QLabel("Smallest rod:"))
        self.min_rod_spin = QSpinBox(minimum=0, maximum=5000, suffix=" px")
        self.min_rod_spin.setSpecialValueText("any")
        self.min_rod_spin.setToolTip(
            "Panda rod outlines: round things with a smaller radius than this (image pixels) are never\n"
            "taken for a stress rod, e.g. dust, bubbles or the core. \"any\" (0) tries every size;\n"
            "raise it when a speck is outlined instead of a rod. Each fiber gets it as a share of\n"
            "its own radius (18% to 40%).")
        self.min_rod_spin.valueChanged.connect(lambda: self._tune_timer.start())
        rod_row.addWidget(self.min_rod_spin, stretch=1)
        self.draw_rod_button = QPushButton("Draw")
        self.draw_rod_button.setCheckable(True)
        self.draw_rod_button.setToolTip("Draw it on the view: click the centre of the smallest rod (or anything that\n"
                                        "size), then its edge. Esc cancels.")
        self.draw_rod_button.toggled.connect(self.draw_rod_requested)
        rod_row.addWidget(self.draw_rod_button)
        advanced.addLayout(rod_row)
        self.parts_centre_check = QCheckBox("Turn about stress parts")
        self.parts_centre_check.setToolTip(
            "Measure the axis about the centre of the stress parts instead of the fiber centre\n"
            "(Mirror symmetry and Second moments; for stress parts off the fiber's middle).")
        self.parts_centre_check.toggled.connect(self.method_changed)
        advanced.addWidget(self.parts_centre_check)
        self.use_circles = QCheckBox("Use circle measurements")
        self.use_circles.setChecked(True)
        self.use_circles.setToolTip("If there are Circle measurements, measure the fibers inside them\n"
                                    "instead of finding fibers automatically.")
        advanced.addWidget(self.use_circles)
        left.addWidget(self.advanced)
        left.addStretch()
        layout.addLayout(left)
        self.detection_box = self._build_detection()
        layout.addWidget(self.detection_box)
        self.method_combo.currentIndexChanged.connect(self._update_detection_enabled)
        self._update_detection_enabled()
        self._show_advanced(False)

        right = QVBoxLayout()
        self.table = QTableWidget(0, 10)
        self.table.setHorizontalHeaderLabels(["#", "Type", "Slow axis (°)", "Fast axis (°)", "Clarity", "Method",
                                              "Centre (px)", "Axis through (px)", "Diameter (px)", "Centre brightness"])
        self.table.setSelectionBehavior(QAbstractItemView.SelectionBehavior.SelectRows)
        self.table.setSelectionMode(QAbstractItemView.SelectionMode.SingleSelection)
        self.table.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers)
        self.table.verticalHeader().setVisible(False)
        self.table.horizontalHeader().setSectionResizeMode(QHeaderView.ResizeMode.ResizeToContents)
        self.table.horizontalHeader().setStretchLastSection(True)
        self.table.itemSelectionChanged.connect(self.changed)
        self.table.horizontalHeaderItem(4).setToolTip(
            "How much longer the stress parts spread along the slow axis than across it.\n"
            "Below 1.3 the axis isn't clear (e.g. a plain round fiber, or poor contrast).\n"
            "Panda rod outlines: how much of the rods' outlines was found;\n"
            "below 50% (or with faint outlines) the axis isn't clear.")
        self.table.horizontalHeaderItem(9).setToolTip(
            "Brightness (0-255) of the brightest spot near the fiber's centre, e.g. light in the core.\n"
            "With several fibers, the brightest is ringed in green and named in the readout\n"
            "by its position counted from the right.")
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
        defaults.clicked.connect(lambda: self.set_detection(Detection(min_rod_px=self.min_rod_spin.value())))
        form.addRow(defaults)
        return box

    def _show_advanced(self, on: bool):
        self.advanced.setVisible(on)
        self.detection_box.setVisible(on)

    def _update_detection_enabled(self):
        # "Panda rod outlines" works on edges: the detection settings don't apply.
        self.detection_box.setEnabled(self.method() != "rods")
        self.parts_centre_check.setEnabled(self.method() != "rods")  # its axis always goes through the rods

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
            min_size=float(self.min_size_slider.value()),
            min_rod=self._min_rod,
            min_rod_px=float(self.min_rod_spin.value()))

    def set_detection(self, d: Detection):
        """Show d's settings (one re-measure, not one per control)."""
        widgets = (self.look_combo, self.auto_check, self.threshold_slider, self.smoothing_slider,
                   self.min_size_slider, self.min_rod_spin)
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
        self.min_rod_spin.setValue(round(d.min_rod_px))
        self._min_rod = d.min_rod  # % of the radius, from settings saved that way (used while no pixels are set)
        for w in widgets:
            w.blockSignals(False)
        self.smoothing_slider.valueChanged.emit(self.smoothing_slider.value())  # update the labels
        self.min_size_slider.valueChanged.emit(self.min_size_slider.value())
        self._tune_timer.stop()  # started by those emits
        self.method_changed.emit()

    def set_min_rod(self, radius_px: float):
        """Smallest rod drawn on the view."""
        self.draw_rod_button.setChecked(False)
        self.min_rod_spin.setValue(round(radius_px))

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
        self.axes = sort_axes(axes)
        keep = self.selected()
        brightest = brightest_centre(self.axes)
        self.table.blockSignals(True)
        self.table.setRowCount(len(self.axes))
        for row, f in enumerate(self.axes):
            light = "" if f.centre_brightness is None else f"{f.centre_brightness:.0f}" + (
                "  (brightest)" if brightest is not None and brightest[0] == row else "")
            values = [str(row + 1), f.fiber_type, fmt_angle(f.slow_deg), fmt_angle(f.fast_deg),
                      clarity_text(f) + ("" if f.clear else " (unclear)"), METHODS[f.method],
                      f"{f.cx:.1f}, {f.cy:.1f}", "{:.1f}, {:.1f}".format(*f.axis_point), f"{2 * f.radius:.1f}",
                      light]
            for col, text in enumerate(values):
                item = QTableWidgetItem(text)
                if col in (2, 3, 4, 8, 9):
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
                 "Method": METHODS[f.method], "Clear": f.clear,
                 "Threshold (noise x)": None if f.method == "rods" else round(f.threshold, 2),
                 "Rod outline found": None if f.coverage is None else round(f.coverage, 3),
                 "Centre x (px)": round(f.cx, 1), "Centre y (px)": round(f.cy, 1),
                 "Axis x (px)": round(f.axis_point[0], 1), "Axis y (px)": round(f.axis_point[1], 1),
                 "Diameter (px)": round(2 * f.radius, 1),
                 "Centre brightness (0-255)": None if f.centre_brightness is None else round(f.centre_brightness, 1),
                 "Image": source}
                for i, f in enumerate(self.axes)]
