"""How the View > Grid and View > Crosshair overlays and the fiber axis lines
look, and the small window (one tab each) for setting it. The window stays
open while you adjust, and the overlays change as you go, on every image
shown. Saved with the same keys as the Rust app, so settings files work in
both.
"""

import math
from dataclasses import asdict, dataclass, field, fields

from PySide6.QtCore import Qt, Signal
from PySide6.QtGui import QColor
from PySide6.QtWidgets import (
    QCheckBox,
    QColorDialog,
    QComboBox,
    QDialog,
    QDialogButtonBox,
    QDoubleSpinBox,
    QFormLayout,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QSlider,
    QSpinBox,
    QTabWidget,
    QVBoxLayout,
    QWidget,
)

LINE_STYLES = {"solid": "Solid", "dash": "Dashed", "dot": "Dotted"}  # name: label
CROSSHAIR_ANCHORS = {  # name: label
    "image": "Image centre (moves with the image)",
    "window": "Window centre (stays put when zooming and panning)",
    "fiber": "Fiber centre (follows the selected fiber, else fiber 1)",
}
SHOW_AXES = {"slow": "Slow", "fast": "Fast", "both": "Both"}  # which fiber axes are shown


@dataclass
class _LineStyle:
    """What every overlay line has: width (screen px, the same at any zoom),
    color, opacity (%) and style (see LINE_STYLES)."""
    width: float = 1.0
    color: str = "#ffffff"
    opacity: int = 100
    line: str = "solid"

    def qcolor(self) -> QColor:
        c = QColor(self.color)
        c.setAlphaF(self.opacity / 100)
        return c

    def pen_style(self) -> Qt.PenStyle:
        return {"dash": Qt.PenStyle.DashLine, "dot": Qt.PenStyle.DotLine}.get(self.line, Qt.PenStyle.SolidLine)

    @classmethod
    def from_dict(cls, data: dict):
        """From saved settings, ignoring unknown keys and bad values."""
        style = cls()
        for f in fields(cls):
            if f.name in data:
                try:
                    setattr(style, f.name, type(getattr(style, f.name))(data[f.name]))
                except (TypeError, ValueError):
                    pass
        default = cls()
        if style.line not in LINE_STYLES:
            style.line = default.line
        if not QColor(style.color).isValid():
            style.color = default.color
        style.width = min(max(style.width, 0.5), 10.0)
        style.opacity = min(max(style.opacity, 5), 100)
        style._check()
        return style

    def _check(self):
        """Put any other out-of-range values right (subclasses)."""

    def to_dict(self) -> dict:
        return asdict(self)

    def copy(self):
        return type(self)(**self.to_dict())


@dataclass
class GridStyle(_LineStyle):
    color: str = "#ffff00"
    columns: int = 8
    rows: int = 8
    square: bool = False  # rows worked out from the image shape, so the cells are square

    def rows_for(self, width: int, height: int) -> int:
        """Rows to draw on an image of this size."""
        if self.square:
            return max(1, round(self.columns * height / width))
        return self.rows

    def _check(self):
        self.columns, self.rows = min(max(self.columns, 1), 100), min(max(self.rows, 1), 100)


@dataclass
class CrosshairStyle(_LineStyle):
    width: float = 2.0
    color: str = "#ff0000"
    circle: int = 20        # radius of the ring round the centre, screen px; 0 = no ring
    anchor: str = "image"   # see CROSSHAIR_ANCHORS

    def _check(self):
        self.circle = min(max(self.circle, 0), 500)
        if self.anchor not in CROSSHAIR_ANCHORS:
            self.anchor = "image"


def _valid_color(c) -> bool:
    return isinstance(c, str) and c.startswith("#") and len(c) == 7 and QColor(c).isValid()


@dataclass
class AngleColors:
    """An axis colored by how close its angle is to its aim (0 or 90 degrees,
    either way: for 90, +89.7 and -89.7 are both 0.3 off): within limits[i]
    (the smallest that fits) it takes colors[i]; beyond them all, the axis's
    own color. Usually the fast axis aims at 0 and the slow axis at 90."""
    on: bool = True
    aim: float = 0.0
    limits: list = field(default_factory=lambda: [0.5, 1.0, 2.0])
    colors: list = field(default_factory=lambda: ["#00ff00", "#ffff00", "#ff8000"])

    @classmethod
    def for_axis(cls, slow: bool) -> "AngleColors":
        """The defaults for the slow axis (aiming at 90) or the fast axis (at 0)."""
        return cls(aim=90.0 if slow else 0.0)

    @classmethod
    def from_dict(cls, data, slow: bool) -> "AngleColors":
        """Field by field, so one bad value doesn't lose the others. Saved before
        there was an aim (all off then): this axis's defaults."""
        s = cls.for_axis(slow)
        if not isinstance(data, dict) or "aim" not in data:
            return s
        if isinstance(data["aim"], (int, float)) and not isinstance(data["aim"], bool):
            s.aim = 90.0 if data["aim"] >= 45 else 0.0
        if isinstance(data.get("on"), bool):
            s.on = data["on"]
        limits, colors = data.get("limits"), data.get("colors")
        for i in range(3):
            v = limits[i] if isinstance(limits, list) and i < len(limits) else None
            if isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v):
                s.limits[i] = min(max(float(v), 0.0), 90.0)
            c = colors[i] if isinstance(colors, list) and i < len(colors) else None
            if _valid_color(c):
                s.colors[i] = c
        return s

    def bands(self, opacity: int):
        """(aim, [(limit, QColor with this opacity)], smallest limit first), or None when off."""
        if not self.on:
            return None
        bands = []
        for limit, color in sorted(zip(self.limits, self.colors), key=lambda b: b[0]):
            c = QColor(color)
            c.setAlphaF(opacity / 100)
            bands.append((limit, c))
        return self.aim, bands


@dataclass
class FiberLineStyle:
    """The fibers' slow and fast axis lines: which are shown (lines, readout,
    exports, videos), their colors and opacity (also the readout's angle
    colors), and colors by angle."""
    show_axes: str = "both"   # see SHOW_AXES
    slow_color: str = "#ff00c8"
    slow_opacity: int = 100
    fast_color: str = "#ffc800"
    fast_opacity: int = 100
    slow_by_angle: AngleColors = field(default_factory=lambda: AngleColors.for_axis(True))
    fast_by_angle: AngleColors = field(default_factory=lambda: AngleColors.for_axis(False))

    @classmethod
    def from_dict(cls, data) -> "FiberLineStyle":
        s = cls()
        if not isinstance(data, dict):
            return s
        if data.get("show_axes") in SHOW_AXES:
            s.show_axes = data["show_axes"]
        for key in ("slow_color", "fast_color"):
            if _valid_color(data.get(key)):
                setattr(s, key, data[key])
        for key in ("slow_opacity", "fast_opacity"):
            v = data.get(key)
            if isinstance(v, (int, float)) and not isinstance(v, bool):
                setattr(s, key, min(max(int(v), 5), 100))
        s.slow_by_angle = AngleColors.from_dict(data.get("slow_by_angle"), True)
        s.fast_by_angle = AngleColors.from_dict(data.get("fast_by_angle"), False)
        return s

    def to_dict(self) -> dict:
        return asdict(self)

    def copy(self) -> "FiberLineStyle":
        return FiberLineStyle.from_dict(self.to_dict())

    def color(self, slow: bool) -> QColor:
        c = QColor(self.slow_color if slow else self.fast_color)
        c.setAlphaF((self.slow_opacity if slow else self.fast_opacity) / 100)
        return c


class _StyleTab(QWidget):
    """The controls for one overlay's style; edited is emitted on every change."""
    edited = Signal()

    def __init__(self, kind: type[_LineStyle], parent=None):
        super().__init__(parent)
        self._kind = kind
        self._style = kind()
        self.form = QFormLayout(self)
        self._extra()

        self.width = QDoubleSpinBox(minimum=0.5, maximum=10.0, singleStep=0.5, decimals=1, suffix=" px")
        self.width.setToolTip("Line width on screen (the same at any zoom)")
        self.form.addRow("Line width:", self.width)
        self.line = QComboBox()
        for name, label in LINE_STYLES.items():
            self.line.addItem(label, name)
        self.form.addRow("Line style:", self.line)
        self.color_button = QPushButton()
        self.color_button.setToolTip("Choose the line color")
        self.color_button.clicked.connect(self._choose_color)
        self.form.addRow("Color:", self.color_button)
        self.opacity = QSlider(Qt.Orientation.Horizontal, minimum=5, maximum=100)
        self.opacity_label = QLabel()
        self.opacity_label.setMinimumWidth(self.opacity_label.fontMetrics().horizontalAdvance("100%"))
        row = QHBoxLayout()
        row.addWidget(self.opacity, stretch=1)
        row.addWidget(self.opacity_label)
        self.form.addRow("Opacity:", row)

        defaults = QPushButton("Defaults")
        defaults.setToolTip("Back to the standard look")
        defaults.clicked.connect(lambda: (self.set_style(kind()), self.edited.emit()))
        self.form.addRow(defaults)
        for signal in self._signals() + [self.width.valueChanged, self.line.currentIndexChanged,
                                         self.opacity.valueChanged]:
            signal.connect(self._on_edit)

    # Subclasses add their own rows and keep them in step with the style.
    def _extra(self):
        pass

    def _signals(self) -> list:
        return []

    def _show_extra(self, style):
        pass

    def _read_extra(self, style):
        pass

    def _widgets(self) -> list:
        return [self.width, self.line, self.opacity]

    def set_style(self, style: _LineStyle):
        """Show style's values, without emitting edited."""
        self._style = style.copy()
        widgets = self._widgets()
        for w in widgets:
            w.blockSignals(True)
        self.width.setValue(style.width)
        self.line.setCurrentIndex(max(0, self.line.findData(style.line)))
        self.opacity.setValue(style.opacity)
        self._show_extra(style)
        for w in widgets:
            w.blockSignals(False)
        self._update_labels()

    def style(self):
        return self._style.copy()

    def _on_edit(self):
        s = self._style
        s.width, s.line, s.opacity = self.width.value(), self.line.currentData(), self.opacity.value()
        self._read_extra(s)
        self._update_labels()
        self.edited.emit()

    def _update_labels(self):
        self.opacity_label.setText(f"{self.opacity.value()}%")
        c = QColor(self._style.color)
        self.color_button.setText(c.name())
        self.color_button.setStyleSheet(f"background: {c.name()}; color: {'black' if c.lightnessF() > 0.5 else 'white'};")

    def _choose_color(self):
        """The overlay follows the color picked as it changes; Cancel puts it back."""
        before = self._style.color
        dialog = QColorDialog(QColor(before), self)
        dialog.setWindowTitle("Line color")

        def preview(color: QColor):
            if color.isValid() and color.name() != self._style.color:
                self._style.color = color.name()
                self._update_labels()
                self.edited.emit()
        dialog.currentColorChanged.connect(preview)
        preview(dialog.selectedColor() if dialog.exec() == QDialog.DialogCode.Accepted else QColor(before))


class _GridTab(_StyleTab):
    def __init__(self, parent=None):
        super().__init__(GridStyle, parent)

    def _extra(self):
        self.columns = QSpinBox(minimum=1, maximum=100)
        self.columns.setToolTip("Number of columns (vertical lines + 1)")
        self.form.addRow("Columns:", self.columns)
        self.rows = QSpinBox(minimum=1, maximum=100)
        self.rows.setToolTip("Number of rows (horizontal lines + 1)")
        self.square = QCheckBox("Square cells")
        self.square.setToolTip("Work out the rows from the image's shape, so every cell is square")
        row = QHBoxLayout()
        row.addWidget(self.rows)
        row.addWidget(self.square)
        self.form.addRow("Rows:", row)

    def _signals(self):
        return [self.columns.valueChanged, self.rows.valueChanged, self.square.toggled]

    def _widgets(self):
        return super()._widgets() + [self.columns, self.rows, self.square]

    def _show_extra(self, style: GridStyle):
        self.columns.setValue(style.columns)
        self.rows.setValue(style.rows)
        self.square.setChecked(style.square)

    def _read_extra(self, style: GridStyle):
        style.columns, style.rows, style.square = self.columns.value(), self.rows.value(), self.square.isChecked()

    def _update_labels(self):
        super()._update_labels()
        self.rows.setEnabled(not self.square.isChecked())


class _CrosshairTab(_StyleTab):
    def __init__(self, parent=None):
        super().__init__(CrosshairStyle, parent)

    def _extra(self):
        self.anchor = QComboBox()
        for name, label in CROSSHAIR_ANCHORS.items():
            self.anchor.addItem(label, name)
        self.anchor.setToolTip("Image centre: marks the middle of the picture (the camera's optical axis), e.g.\n"
                               "to centre a fiber. Window centre: marks the middle of what you're looking at,\n"
                               "wherever you've zoomed or panned to.")
        self.form.addRow("Centred on:", self.anchor)
        self.circle = QSpinBox(minimum=0, maximum=500, suffix=" px")
        self.circle.setSpecialValueText("None")
        self.circle.setToolTip("Radius of the ring round the centre, on screen (0: no ring)")
        self.form.addRow("Ring:", self.circle)

    def _signals(self):
        return [self.anchor.currentIndexChanged, self.circle.valueChanged]

    def _widgets(self):
        return super()._widgets() + [self.anchor, self.circle]

    def _show_extra(self, style: CrosshairStyle):
        self.anchor.setCurrentIndex(max(0, self.anchor.findData(style.anchor)))
        self.circle.setValue(style.circle)

    def _read_extra(self, style: CrosshairStyle):
        style.anchor, style.circle = self.anchor.currentData(), self.circle.value()


def _color_button(color: str, edited) -> QPushButton:
    """A button showing a color; clicking it picks another (edited(name) as it changes)."""
    button = QPushButton()
    button.setToolTip("Choose the color")

    def show(c: str):
        q = QColor(c)
        button.setText(q.name())
        button.setStyleSheet(f"background: {q.name()}; color: {'black' if q.lightnessF() > 0.5 else 'white'};")
        button.setProperty("color", q.name())

    def choose():
        before = button.property("color")
        dialog = QColorDialog(QColor(before), button)
        dialog.setWindowTitle("Color")

        def preview(c: QColor):
            if c.isValid() and c.name() != button.property("color"):
                show(c.name())
                edited(c.name())
        dialog.currentColorChanged.connect(preview)
        preview(dialog.selectedColor() if dialog.exec() == QDialog.DialogCode.Accepted else QColor(before))
    button.clicked.connect(choose)
    button.show_color = show
    show(color)
    return button


class _AxisRows:
    """One axis's rows on the Fiber axes tab: color, opacity, colors by angle."""

    def __init__(self, form: QFormLayout, name: str, slow: bool, edited):
        self.slow = slow
        self._edited = edited
        self.color = _color_button("#ffffff", lambda c: self._set("color", c))
        form.addRow(f"{name} axis:", self.color)
        self.opacity = QSlider(Qt.Orientation.Horizontal, minimum=5, maximum=100)
        self.opacity_label = QLabel()
        self.opacity_label.setMinimumWidth(self.opacity_label.fontMetrics().horizontalAdvance("100%"))
        row = QHBoxLayout()
        row.addWidget(self.opacity, stretch=1)
        row.addWidget(self.opacity_label)
        form.addRow("Opacity:", row)
        self.by_angle = QCheckBox("Color by angle")
        self.by_angle.setToolTip(
            f"Color the {name.lower()} axis by how close its angle is to the aim (0° or 90°, either way: for 90°,\n"
            "+89.7° and -89.7° are both 0.3° off): within the first limit, its first color; else within\n"
            "the second, the second color; else within the third, the third. Beyond them all, the axis\n"
            "color above. The readout's angle takes the same color. Usually the fast axis aims at 0°\n"
            "and the slow axis at 90°.")
        form.addRow("", self.by_angle)
        self.aim = QComboBox()
        self.aim.addItem("0°", 0.0)
        self.aim.addItem("±90°", 90.0)
        self.aim_label = QLabel("Aim for:")
        form.addRow(self.aim_label, self.aim)
        self.limits, self.band_colors, self.band_rows = [], [], []
        for i in range(3):
            limit = QDoubleSpinBox(minimum=0.0, maximum=90.0, singleStep=0.05, decimals=2, suffix="°")
            of = QLabel()
            color = _color_button("#ffffff", lambda c, i=i: self._set_band(i, c))
            row = QWidget()
            h = QHBoxLayout(row)
            h.setContentsMargins(0, 0, 0, 0)
            h.addWidget(QLabel("within"))
            h.addWidget(limit)
            h.addWidget(of)
            h.addWidget(color)
            h.addStretch()
            form.addRow("", row)
            limit.valueChanged.connect(self._on_edit)
            self.limits.append((limit, of))
            self.band_colors.append(color)
            self.band_rows.append(row)
        self.opacity.valueChanged.connect(self._on_edit)
        self.by_angle.toggled.connect(self._on_edit)
        self.aim.currentIndexChanged.connect(self._on_edit)
        self.style: FiberLineStyle | None = None

    def widgets(self) -> list:
        return [self.opacity, self.by_angle, self.aim] + [w for w, _ in self.limits]

    def angles(self) -> AngleColors:
        return self.style.slow_by_angle if self.slow else self.style.fast_by_angle

    def show(self, style: FiberLineStyle):
        self.style = style
        prefix = "slow" if self.slow else "fast"
        self.color.show_color(getattr(style, f"{prefix}_color"))
        self.opacity.setValue(getattr(style, f"{prefix}_opacity"))
        a = self.angles()
        self.by_angle.setChecked(a.on)
        self.aim.setCurrentIndex(1 if a.aim >= 45 else 0)
        for (spin, _), value, button, color in zip(self.limits, a.limits, self.band_colors, a.colors):
            spin.setValue(value)
            button.show_color(color)
        self._update_labels()

    def _update_labels(self):
        prefix = "slow" if self.slow else "fast"
        self.opacity_label.setText(f"{getattr(self.style, f'{prefix}_opacity')}%")
        on = self.by_angle.isChecked()
        for w in [self.aim, self.aim_label] + self.band_rows:
            w.setVisible(on)
        for _, of in self.limits:
            of.setText(f"of {self.aim.currentText()}")

    def _set(self, key: str, value):
        setattr(self.style, f"{'slow' if self.slow else 'fast'}_{key}", value)
        self._update_labels()
        self._edited()

    def _set_band(self, i: int, color: str):
        self.angles().colors[i] = color
        self._edited()

    def _on_edit(self):
        a = self.angles()
        a.on, a.aim = self.by_angle.isChecked(), self.aim.currentData()
        a.limits = [spin.value() for spin, _ in self.limits]
        self._set("opacity", self.opacity.value())


class _FiberTab(QWidget):
    """Fiber axes: which are shown, lines outside the fibers, and their colors."""
    edited = Signal()

    def __init__(self, parent=None):
        super().__init__(parent)
        self._style = FiberLineStyle()
        form = QFormLayout(self)
        self.show_axes = QComboBox()
        for name, label in SHOW_AXES.items():
            self.show_axes.addItem(label, name)
        self.show_axes.setToolTip("Which axes are drawn on the views, exports and videos, and read out.")
        form.addRow("Show:", self.show_axes)
        self.outside = QCheckBox("Outside the fiber")
        self.outside.setToolTip(
            "Draw each fiber's axis lines only outside the fibers, from the fiber's edge to the edges of the view\n"
            "(as the crosshair), so they can be compared with the crosshair, the grid or lines in the image\n"
            "without covering the fiber. With the crosshair centred on the fiber, its lines also stop at the\n"
            "fibers' edges. Also used for exported drawings (to the picture's edges) and videos.")
        form.addRow("Lines:", self.outside)
        self.slow_rows = _AxisRows(form, "Slow", True, self._emit)
        self.fast_rows = _AxisRows(form, "Fast", False, self._emit)
        note = QLabel("The colors are also used for the angles in the readout.")
        note.setStyleSheet("color: gray;")
        form.addRow(note)
        defaults = QPushButton("Defaults")
        defaults.setToolTip("Back to the standard look")
        defaults.clicked.connect(lambda: (self.set_style(False, FiberLineStyle()), self.edited.emit()))
        form.addRow(defaults)
        self.show_axes.currentIndexChanged.connect(self._on_edit)
        self.outside.toggled.connect(self._on_edit)

    def _widgets(self) -> list:
        return [self.show_axes, self.outside] + self.slow_rows.widgets() + self.fast_rows.widgets()

    def set_style(self, outside: bool, style: FiberLineStyle):
        """Show these values, without emitting edited."""
        self._style = style.copy()
        widgets = self._widgets()
        for w in widgets:
            w.blockSignals(True)
        self.show_axes.setCurrentIndex(max(0, self.show_axes.findData(style.show_axes)))
        self.outside.setChecked(outside)
        self.slow_rows.show(self._style)
        self.fast_rows.show(self._style)
        for w in widgets:
            w.blockSignals(False)

    def style(self) -> tuple[bool, FiberLineStyle]:
        return self.outside.isChecked(), self._style.copy()

    def _on_edit(self):
        self._style.show_axes = self.show_axes.currentData()
        self._emit()

    def _emit(self):
        self.edited.emit()


class OverlayDialog(QDialog):
    """Non-modal: changed is emitted on every edit, so the overlays follow live."""
    changed = Signal(object, object, bool, object)  # GridStyle, CrosshairStyle, outside lines, FiberLineStyle
    TABS = ("grid", "crosshair", "fiber")

    def __init__(self, grid: GridStyle, crosshair: CrosshairStyle, outside: bool = False,
                 fiber: FiberLineStyle | None = None, parent=None):
        super().__init__(parent)
        self.setWindowTitle("Grid, crosshair and fiber axes")
        self.setWindowFlag(Qt.WindowType.WindowContextHelpButtonHint, False)
        self.grid_tab, self.crosshair_tab, self.fiber_tab = _GridTab(), _CrosshairTab(), _FiberTab()
        self.tabs = QTabWidget()
        self.tabs.addTab(self.grid_tab, "Grid")
        self.tabs.addTab(self.crosshair_tab, "Crosshair")
        self.tabs.addTab(self.fiber_tab, "Fiber axes")
        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Close)
        buttons.rejected.connect(self.close)
        layout = QVBoxLayout(self)
        layout.addWidget(self.tabs)
        layout.addWidget(buttons)
        self.set_styles(grid, crosshair, outside, fiber or FiberLineStyle())
        for tab in (self.grid_tab, self.crosshair_tab, self.fiber_tab):
            tab.edited.connect(lambda: self.changed.emit(*self.styles()))

    def show_tab(self, name: str):
        self.tabs.setCurrentIndex(self.TABS.index(name) if name in self.TABS else 0)

    def current_tab(self) -> str:
        return self.TABS[self.tabs.currentIndex()]

    def set_styles(self, grid: GridStyle, crosshair: CrosshairStyle, outside: bool, fiber: FiberLineStyle):
        self.grid_tab.set_style(grid)
        self.crosshair_tab.set_style(crosshair)
        self.fiber_tab.set_style(outside, fiber)

    def styles(self) -> tuple[GridStyle, CrosshairStyle, bool, FiberLineStyle]:
        return self.grid_tab.style(), self.crosshair_tab.style(), *self.fiber_tab.style()
