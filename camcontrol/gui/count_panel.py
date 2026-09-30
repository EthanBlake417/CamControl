"""Counting panel: pick a class, click objects on the image to count them.

Owns the Counter (marks and class names). The image view draws the marks
and reports clicks; the main window connects the two.
"""

from PySide6.QtCore import QPointF, Qt, Signal
from PySide6.QtGui import QColor, QImage, QPainter, QPen
from PySide6.QtWidgets import (
    QButtonGroup,
    QGridLayout,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QMessageBox,
    QPushButton,
    QRadioButton,
    QVBoxLayout,
    QWidget,
)

from camcontrol.gui.qt_image import to_qimage
from camcontrol.processing.count import CLASS_COLORS, MAX_CLASSES, Counter

HINT = ("Left-click adds a mark of the selected class. Right-click removes the nearest mark. "
        "Backspace undoes the last one, Esc stops counting. Middle-drag pans.")


def class_color(cls: int) -> QColor:
    return QColor(*CLASS_COLORS[cls])


def draw_marks(painter: QPainter, counter: Counter, to_out, radius: float):
    """A filled dot per mark, in its class color with a dark outline."""
    pen = QPen(QColor(0, 0, 0), max(1.0, radius / 4))
    painter.setPen(pen)
    for m in counter.marks:
        painter.setBrush(class_color(m.cls))
        painter.drawEllipse(to_out(QPointF(m.x, m.y)), radius, radius)


def render_marks(image, counter: Counter) -> QImage:
    """A full-resolution copy of the image with the marks drawn on it."""
    qimg = to_qimage(image).convertToFormat(QImage.Format.Format_RGB32)
    painter = QPainter(qimg)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    draw_marks(painter, counter, lambda p: p, max(5.0, image.shape[1] / 300))
    painter.end()
    return qimg


class CountPanel(QWidget):
    mode_changed = Signal(object)  # selected class index while counting, or None
    changed = Signal()             # marks or names changed: redraw
    export_requested = Signal()

    def __init__(self, parent=None):
        super().__init__(parent)
        self.counter = Counter()
        layout = QHBoxLayout(self)

        left = QVBoxLayout()
        self.count_button = QPushButton("Count")
        self.count_button.setCheckable(True)
        self.count_button.setToolTip("Turn on to add marks by clicking the image.")
        self.count_button.toggled.connect(lambda _: self._emit_mode())
        left.addWidget(self.count_button)
        undo = QPushButton("Undo")
        undo.clicked.connect(self.undo)
        clear = QPushButton("Clear all")
        clear.clicked.connect(self.clear)
        export = QPushButton("Export...")
        export.setToolTip("Save the counts as Excel or CSV, plus an image with the marks drawn on.")
        export.clicked.connect(self.export_requested)
        for b in (undo, clear, export):
            left.addWidget(b)
        left.addStretch()
        layout.addLayout(left)

        # One row per class: select, color, name, count.
        right = QVBoxLayout()
        grid = QGridLayout()
        self._class_group = QButtonGroup(self)
        self._name_edits: list[QLineEdit] = []
        self._count_labels: list[QLabel] = []
        for i in range(MAX_CLASSES):
            radio = QRadioButton()
            self._class_group.addButton(radio, i)
            swatch = QLabel()
            swatch.setFixedSize(16, 16)
            swatch.setStyleSheet(f"background: {class_color(i).name()}; border: 1px solid black;")
            name = QLineEdit(self.counter.names[i])
            name.textChanged.connect(lambda text, i=i: self._rename(i, text))
            count = QLabel("0")
            count.setMinimumWidth(80)
            count.setAlignment(Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter)
            for col, w in enumerate((radio, swatch, name, count)):
                grid.addWidget(w, i, col)
            self._name_edits.append(name)
            self._count_labels.append(count)
        self._class_group.button(0).setChecked(True)
        self._class_group.idToggled.connect(lambda _id, on: on and self._emit_mode())
        self.total_label = QLabel()
        grid.addWidget(self.total_label, MAX_CLASSES, 2, 1, 2, Qt.AlignmentFlag.AlignRight)
        right.addLayout(grid)
        hint = QLabel(HINT)
        hint.setWordWrap(True)
        hint.setStyleSheet("color: gray;")
        right.addWidget(hint)
        right.addStretch()
        layout.addLayout(right, stretch=1)

        self._refresh()

    # --- mode ---------------------------------------------------------------------------

    @property
    def active_class(self) -> int | None:
        return self._class_group.checkedId() if self.count_button.isChecked() else None

    def set_counting(self, on: bool):
        self.count_button.setChecked(on)

    def _emit_mode(self):
        self.mode_changed.emit(self.active_class)

    # --- marks ----------------------------------------------------------------------------

    def add(self, x: float, y: float):
        if self.active_class is None:
            return
        self.counter.add(x, y, self.active_class)
        self._refresh()

    def remove_near(self, x: float, y: float, max_distance: float):
        if self.counter.remove_nearest(x, y, max_distance):
            self._refresh()

    def undo(self):
        if self.counter.marks:
            self.counter.marks.pop()
            self._refresh()

    def clear(self, confirm: bool = True):
        if not self.counter.marks:
            return
        if confirm and QMessageBox.question(
            self, "Clear marks", f"Delete all {len(self.counter.marks)} marks?"
        ) != QMessageBox.StandardButton.Yes:
            return
        self.counter.marks.clear()
        self._refresh()

    def _rename(self, i: int, text: str):
        self.counter.names[i] = text.strip() or f"Class {i + 1}"

    def _refresh(self):
        counts = self.counter.counts()
        total = sum(counts)
        for label, n in zip(self._count_labels, counts):
            label.setText(f"{n}  ({100 * n / total:.0f}%)" if total else "0")
        self.total_label.setText(f"Total: {total}")
        self.changed.emit()
