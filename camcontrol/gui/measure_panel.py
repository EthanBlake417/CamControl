"""Measurements panel: tool buttons, results table, delete/clear/export.

Owns the list of measurements and the calibration they're shown in. The
image view draws them; the main window connects the two.
"""

from PySide6.QtCore import Qt, Signal
from PySide6.QtWidgets import (
    QAbstractItemView,
    QButtonGroup,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QMessageBox,
    QPushButton,
    QSizePolicy,
    QTableWidget,
    QTableWidgetItem,
    QToolButton,
    QVBoxLayout,
    QWidget,
)

from camcontrol.calibration import PIXELS, Calibration
from camcontrol.measure import KINDS, Measurement, compute, fmt

HINT = ("Click points on the image. Polyline/polygon: double-click, right-click or Enter to finish. "
        "Backspace undoes a point, Esc cancels. Middle-drag pans.")


class MeasurePanel(QWidget):
    tool_changed = Signal(object)       # tool name (key of KINDS) or None
    selection_changed = Signal(object)  # measurement id or None
    changed = Signal()                  # list or calibration changed: redraw
    export_requested = Signal()

    def __init__(self, parent=None):
        super().__init__(parent)
        self.measurements: list[Measurement] = []
        self.calibration: Calibration = PIXELS
        self._next_id = 1

        layout = QHBoxLayout(self)

        # Tool buttons, one per kind plus "Pan" (no tool).
        tools = QVBoxLayout()
        self._tool_group = QButtonGroup(self)
        self._tool_group.setExclusive(True)
        self._tool_buttons: dict[str | None, QToolButton] = {}
        for key, label in [(None, "Pan")] + [(k.name, k.label) for k in KINDS.values()]:
            b = QToolButton()
            b.setText(label)
            b.setCheckable(True)
            b.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
            b.clicked.connect(lambda _=False, k=key: self.set_tool(k))
            self._tool_group.addButton(b)
            self._tool_buttons[key] = b
            tools.addWidget(b)
        self._tool_buttons[None].setChecked(True)
        tools.addStretch()
        layout.addLayout(tools)

        # Results table.
        middle = QVBoxLayout()
        self.table = QTableWidget(0, 6)
        self.table.setSelectionBehavior(QAbstractItemView.SelectionBehavior.SelectRows)
        self.table.setSelectionMode(QAbstractItemView.SelectionMode.SingleSelection)
        self.table.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers)
        self.table.verticalHeader().setVisible(False)
        self.table.horizontalHeader().setSectionResizeMode(QHeaderView.ResizeMode.ResizeToContents)
        self.table.horizontalHeader().setStretchLastSection(True)
        self.table.itemSelectionChanged.connect(lambda: self.selection_changed.emit(self.selected_id()))
        middle.addWidget(self.table)
        hint = QLabel(HINT)
        hint.setWordWrap(True)
        hint.setStyleSheet("color: gray;")
        middle.addWidget(hint)
        layout.addLayout(middle, stretch=1)

        # Buttons.
        buttons = QVBoxLayout()
        delete = QPushButton("Delete")
        delete.setToolTip("Delete the selected measurement (or press Delete in the table).")
        delete.clicked.connect(self.delete_selected)
        clear = QPushButton("Clear all")
        clear.clicked.connect(self.clear)
        export = QPushButton("Export...")
        export.setToolTip("Save the table as Excel or CSV, plus the image and an annotated copy.")
        export.clicked.connect(self.export_requested)
        for b in (delete, clear, export):
            buttons.addWidget(b)
        buttons.addStretch()
        layout.addLayout(buttons)

        self._refresh_table()

    # --- tools -------------------------------------------------------------------

    def set_tool(self, key: str | None):
        self._tool_buttons[key].setChecked(True)
        self.tool_changed.emit(key)

    # --- measurements -------------------------------------------------------------

    def add(self, kind: str, points: list) -> str | None:
        """Add a measurement. Returns an error message if the shape is invalid."""
        m = Measurement(self._next_id, kind, list(points))
        try:
            compute(m, self.calibration)
        except ValueError as e:
            return f"{KINDS[kind].label} not added: {e}"
        self._next_id += 1
        self.measurements.append(m)
        self._refresh_table()
        self.table.selectRow(len(self.measurements) - 1)
        self.changed.emit()
        return None

    def selected_id(self) -> int | None:
        rows = self.table.selectionModel().selectedRows()
        if not rows:
            return None
        return self.measurements[rows[0].row()].id

    def delete_selected(self):
        sel = self.selected_id()
        if sel is None:
            return
        # Mutate in place: the image view holds a reference to this list.
        self.measurements[:] = [m for m in self.measurements if m.id != sel]
        self._refresh_table()
        self.changed.emit()

    def clear(self, confirm: bool = True):
        if not self.measurements:
            return
        if confirm and QMessageBox.question(
            self, "Clear measurements", f"Delete all {len(self.measurements)} measurements?"
        ) != QMessageBox.StandardButton.Yes:
            return
        self.measurements.clear()
        self._next_id = 1
        self._refresh_table()
        self.changed.emit()

    def set_calibration(self, cal: Calibration):
        self.calibration = cal
        self._refresh_table()
        self.changed.emit()

    def keyPressEvent(self, event):
        if event.key() == Qt.Key.Key_Delete:
            self.delete_selected()
            return
        super().keyPressEvent(event)

    # --- table ----------------------------------------------------------------------

    def _refresh_table(self):
        u = self.calibration.unit
        self.table.setHorizontalHeaderLabels(
            ["#", "Type", f"Length / perim. ({u})", f"Area ({u}²)", "Angle (°)", "Details"]
        )
        self.table.blockSignals(True)
        self.table.setRowCount(len(self.measurements))
        for row, m in enumerate(self.measurements):
            r = compute(m, self.calibration)
            length = r.get("length", r.get("perimeter"))
            details = ""
            if m.kind == "circle":
                cx, cy = r["center_px"]
                details = f"⌀ {fmt(r['diameter'])}, r {fmt(r['radius'])} {u}; centre ({cx:.1f}, {cy:.1f}) px"
            elif m.kind == "rectangle":
                details = f"{fmt(r['width'])} × {fmt(r['height'])} {u}"
            elif m.kind in ("polyline", "polygon"):
                details = f"{len(m.points)} points"
            values = [
                str(m.id),
                KINDS[m.kind].label,
                fmt(length) if length is not None else "",
                fmt(r["area"]) if "area" in r else "",
                fmt(r["angle_deg"]) if "angle_deg" in r else "",
                details,
            ]
            for col, text in enumerate(values):
                item = QTableWidgetItem(text)
                if 0 < col < 5 and col != 1:
                    item.setTextAlignment(Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter)
                self.table.setItem(row, col, item)
        self.table.blockSignals(False)
        self.selection_changed.emit(self.selected_id())
