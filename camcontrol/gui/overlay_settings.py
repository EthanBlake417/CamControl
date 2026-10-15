"""How the View > Grid and View > Crosshair overlays look, and the small
window (one tab each) for setting it. The window stays open while you
adjust, and the overlays change as you go, on every image shown.
"""

from dataclasses import asdict, dataclass, fields

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
}


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


class OverlayDialog(QDialog):
    """Non-modal: changed is emitted on every edit, so the overlays follow live."""
    changed = Signal(object, object)  # GridStyle, CrosshairStyle

    def __init__(self, grid: GridStyle, crosshair: CrosshairStyle, parent=None):
        super().__init__(parent)
        self.setWindowTitle("Grid and crosshair")
        self.setWindowFlag(Qt.WindowType.WindowContextHelpButtonHint, False)
        self.grid_tab, self.crosshair_tab = _GridTab(), _CrosshairTab()
        self.tabs = QTabWidget()
        self.tabs.addTab(self.grid_tab, "Grid")
        self.tabs.addTab(self.crosshair_tab, "Crosshair")
        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Close)
        buttons.rejected.connect(self.close)
        layout = QVBoxLayout(self)
        layout.addWidget(self.tabs)
        layout.addWidget(buttons)
        self.set_styles(grid, crosshair)
        for tab in (self.grid_tab, self.crosshair_tab):
            tab.edited.connect(lambda: self.changed.emit(*self.styles()))

    def show_tab(self, name: str):
        self.tabs.setCurrentWidget(self.crosshair_tab if name == "crosshair" else self.grid_tab)

    def set_styles(self, grid: GridStyle, crosshair: CrosshairStyle):
        self.grid_tab.set_style(grid)
        self.crosshair_tab.set_style(crosshair)

    def styles(self) -> tuple[GridStyle, CrosshairStyle]:
        return self.grid_tab.style(), self.crosshair_tab.style()
