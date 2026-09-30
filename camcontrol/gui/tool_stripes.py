"""PyCharm-style panel stripes: a thin bar along the left, right and bottom
edges of the window, with one button per panel (just its name).

- Click a panel's button to open it, or to minimize it (its space goes to
  the image). Each edge shows one panel at a time: opening a panel
  minimizes the others on the same edge.
- Drag a button to another stripe to move the panel to that edge, or along
  its stripe to change the order. (While dragging, all three stripes show,
  even empty ones, so any edge can be picked.)
- Each panel's title bar has a "minimize" button.
- Stripes with no buttons are hidden.
"""

import json

from PySide6.QtCore import QMimeData, QPoint, QRect, QSize, Qt, QTimer
from PySide6.QtGui import QDrag
from PySide6.QtWidgets import (
    QApplication,
    QDockWidget,
    QHBoxLayout,
    QLabel,
    QMainWindow,
    QStyle,
    QStyleOptionToolButton,
    QStylePainter,
    QToolBar,
    QToolButton,
    QWidget,
)

Area = Qt.DockWidgetArea
MIME_TYPE = "application/x-camcontrol-panel"  # drag data: the panel's objectName
STRIPE_THICKNESS = 24  # px; also the size of an empty stripe while dragging

# Stripe for each dock area. Panels docked at the top (not used by default)
# go on the left stripe.
STRIPE_FOR_AREA = {
    Area.LeftDockWidgetArea: Area.LeftDockWidgetArea,
    Area.TopDockWidgetArea: Area.LeftDockWidgetArea,
    Area.RightDockWidgetArea: Area.RightDockWidgetArea,
    Area.BottomDockWidgetArea: Area.BottomDockWidgetArea,
}
TOOLBAR_AREA = {
    Area.LeftDockWidgetArea: Qt.ToolBarArea.LeftToolBarArea,
    Area.RightDockWidgetArea: Qt.ToolBarArea.RightToolBarArea,
    Area.BottomDockWidgetArea: Qt.ToolBarArea.BottomToolBarArea,
}
# Text direction on the side stripes, as in PyCharm: left reads bottom-to-top,
# right reads top-to-bottom.
ROTATION = {
    Area.LeftDockWidgetArea: -90,
    Area.RightDockWidgetArea: 90,
    Area.BottomDockWidgetArea: 0,
}


class StripeButton(QToolButton):
    """A flat, checkable panel button whose text can be turned sideways.
    Click to open/minimize the panel; drag to move it."""

    def __init__(self, dock: QDockWidget, rotation: int, parent=None):
        super().__init__(parent)
        self.dock = dock
        self.rotation = rotation
        self._press_pos: QPoint | None = None
        self.setText(dock.windowTitle())
        self.setCheckable(True)
        self.setAutoRaise(True)
        self.setToolButtonStyle(Qt.ToolButtonStyle.ToolButtonTextOnly)
        # Open panels get a clear highlight; minimized ones are plain text.
        self.setStyleSheet(
            "QToolButton { padding: 3px 4px; border: 1px solid transparent; border-radius: 3px; }"
            "QToolButton:hover { background: #e2e8f2; }"
            "QToolButton:checked { background: #cad8ef; border-color: #9fb3d6; }"
        )

    def sizeHint(self) -> QSize:
        size = super().sizeHint()
        size = QSize(size.width() + 8, size.height())  # a little room around the name
        return size.transposed() if self.rotation else size

    def minimumSizeHint(self) -> QSize:
        return self.sizeHint()

    def paintEvent(self, event):
        if not self.rotation:
            super().paintEvent(event)
            return
        painter = QStylePainter(self)
        if self.rotation == 90:
            painter.translate(self.width(), 0)
        else:
            painter.translate(0, self.height())
        painter.rotate(self.rotation)
        option = QStyleOptionToolButton()
        self.initStyleOption(option)
        option.rect = QRect(0, 0, self.height(), self.width())  # drawn sideways
        painter.drawComplexControl(QStyle.ComplexControl.CC_ToolButton, option)

    # --- dragging ---------------------------------------------------------------

    def mousePressEvent(self, event):
        if event.button() == Qt.MouseButton.LeftButton:
            self._press_pos = event.position().toPoint()
        super().mousePressEvent(event)

    def mouseMoveEvent(self, event):
        if (self._press_pos is not None and event.buttons() & Qt.MouseButton.LeftButton
                and (event.position().toPoint() - self._press_pos).manhattanLength()
                >= QApplication.startDragDistance()):
            self._press_pos = None
            self.setDown(False)  # a drag, not a click
            stripes: ToolStripes = self.window().stripes
            stripes.drag(self)
            return
        super().mouseMoveEvent(event)

    def mouseReleaseEvent(self, event):
        self._press_pos = None
        super().mouseReleaseEvent(event)


class StripeBar(QToolBar):
    """One edge's stripe. Accepts dropped panel buttons."""

    def __init__(self, title: str, area: Area, stripes: "ToolStripes"):
        super().__init__(title, stripes.window)
        self.area = area
        self.stripes = stripes
        self.setAcceptDrops(True)
        self.setMovable(False)
        self.setFloatable(False)
        self.toggleViewAction().setVisible(False)  # always managed here, never hidden by hand
        self._set_highlight(False)
        if area == Area.BottomDockWidgetArea:
            self.setMinimumHeight(STRIPE_THICKNESS)
        else:
            self.setMinimumWidth(STRIPE_THICKNESS)

    def _set_highlight(self, on: bool):
        background = "background: #cad8ef;" if on else ""
        self.setStyleSheet(f"QToolBar {{ spacing: 2px; padding: 0px; border: none; {background} }}")

    def drop_index(self, pos: QPoint) -> int:
        """Where a button dropped at pos goes among this stripe's buttons."""
        vertical = self.area != Area.BottomDockWidgetArea
        index = 0
        for button in self.findChildren(StripeButton):
            centre = button.geometry().center()
            if (pos.y() > centre.y()) if vertical else (pos.x() > centre.x()):
                index += 1
        return index

    def dragEnterEvent(self, event):
        if event.mimeData().hasFormat(MIME_TYPE):
            self._set_highlight(True)
            event.acceptProposedAction()

    def dragMoveEvent(self, event):
        if event.mimeData().hasFormat(MIME_TYPE):
            event.acceptProposedAction()

    def dragLeaveEvent(self, event):
        self._set_highlight(False)

    def dropEvent(self, event):
        self._set_highlight(False)
        name = bytes(event.mimeData().data(MIME_TYPE)).decode()
        index = self.drop_index(event.position().toPoint())
        event.acceptProposedAction()
        self.stripes.move_to(name, self.area, index)


class DockTitleBar(QWidget):
    """Panel title with a minimize button (replaces Qt's own title bar)."""

    def __init__(self, dock: QDockWidget):
        super().__init__(dock)
        layout = QHBoxLayout(self)
        layout.setContentsMargins(6, 2, 2, 2)
        title = QLabel(dock.windowTitle())
        title.setStyleSheet("font-weight: bold;")
        layout.addWidget(title, stretch=1)
        minimize = QToolButton()
        minimize.setIcon(self.style().standardIcon(QStyle.StandardPixmap.SP_TitleBarMinButton))
        minimize.setAutoRaise(True)
        minimize.setToolTip("Minimize to the edge of the window")
        minimize.clicked.connect(dock.hide)
        layout.addWidget(minimize)


class ToolStripes:
    def __init__(self, window: QMainWindow):
        self.window = window
        self.docks: list[QDockWidget] = []           # in stripe order
        self._area: dict[QDockWidget, Area] = {}     # edge each panel belongs to
        self._buttons: dict[QDockWidget, StripeButton] = {}
        self._refreshing = False
        self.stripes: dict[Area, StripeBar] = {}
        for area, name in ((Area.LeftDockWidgetArea, "left"), (Area.RightDockWidgetArea, "right"),
                           (Area.BottomDockWidgetArea, "bottom")):
            bar = StripeBar(f"{name} panels", area, self)
            bar.setObjectName(f"{name}_stripe")  # for saveState()
            window.addToolBar(TOOLBAR_AREA[area], bar)
            self.stripes[area] = bar

    def add(self, dock: QDockWidget, area: Area):
        self.docks.append(dock)
        self._area[dock] = area
        dock.setTitleBarWidget(DockTitleBar(dock))
        dock.dockLocationChanged.connect(lambda a, d=dock: self._moved(d, a))
        dock.visibilityChanged.connect(lambda _v, d=dock: self._update_checked(d))
        # Opening a panel any other way (shortcut, View > Panels) also
        # minimizes the others on its edge.
        dock.toggleViewAction().triggered.connect(lambda checked, d=dock: checked and self._only(d))
        self.refresh()

    # --- order (saved with the settings) ------------------------------------------

    def order(self) -> str:
        return json.dumps([d.objectName() for d in self.docks])

    def set_order(self, saved: str):
        try:
            names = json.loads(saved)
        except (TypeError, ValueError):
            names = []  # nothing saved yet: keep the order the panels were added in
        rank = {name: i for i, name in enumerate(names)}
        self.docks.sort(key=lambda d: rank.get(d.objectName(), len(rank)))
        self.refresh()

    # --- showing panels ---------------------------------------------------------------

    def refresh(self, prefer: QDockWidget | None = None):
        """Rebuild the stripes from where each panel is now (e.g. after restoring
        a layout), and make sure each edge shows at most one panel."""
        if self._refreshing:
            return
        self._refreshing = True
        try:
            for dock in self.docks:
                area = self.window.dockWidgetArea(dock)
                if area in STRIPE_FOR_AREA:
                    self._area[dock] = STRIPE_FOR_AREA[area]
            for area in self.stripes:
                open_docks = [d for d in self.docks if self._area[d] == area and not d.isHidden()]
                if len(open_docks) > 1:
                    keep = prefer if prefer in open_docks else next(
                        (d for d in open_docks if d.isVisible()), open_docks[0])
                    for d in open_docks:
                        if d is not keep:
                            d.hide()
            for bar in self.stripes.values():
                bar.clear()
            # clear() doesn't delete the buttons (they'd be left floating in
            # the window), so remove them here.
            for button in self._buttons.values():
                button.hide()
                button.deleteLater()
            self._buttons.clear()
            for dock in self.docks:
                area = self._area[dock]
                button = StripeButton(dock, ROTATION[area])
                shortcut = dock.toggleViewAction().shortcut().toString()
                button.setToolTip(f"Open / minimize {dock.windowTitle()}"
                                  + (f" ({shortcut})" if shortcut else "")
                                  + ". Drag to move it to another edge.")
                button.clicked.connect(lambda _c=False, d=dock: self._clicked(d))
                self.stripes[area].addWidget(button)
                self._buttons[dock] = button
                self._update_checked(dock)
            for bar in self.stripes.values():
                bar.setVisible(bool(bar.actions()))
        finally:
            self._refreshing = False

    def _moved(self, dock: QDockWidget, area: Area):
        if area in STRIPE_FOR_AREA and not self._refreshing:
            self.refresh(prefer=dock)

    def _clicked(self, dock: QDockWidget):
        if dock.isHidden() or not dock.isVisible():  # minimized, or behind another panel
            dock.show()
            self._only(dock)
        else:
            dock.hide()
        self._update_checked(dock)

    def _only(self, dock: QDockWidget):
        """Show dock and minimize the other panels on its edge."""
        for other in self.docks:
            if other is not dock and self._area[other] == self._area[dock] and not other.isHidden():
                other.hide()
        dock.raise_()

    def _update_checked(self, dock: QDockWidget):
        button = self._buttons.get(dock)
        if button is not None:
            button.setChecked(not dock.isHidden())

    # --- moving panels by dragging their buttons ---------------------------------------

    def drag(self, button: StripeButton):
        # Show every stripe while dragging, so an empty edge can be chosen too.
        for bar in self.stripes.values():
            bar.setVisible(True)
        mime = QMimeData()
        mime.setData(MIME_TYPE, button.dock.objectName().encode())
        drag = QDrag(button)
        drag.setMimeData(mime)
        drag.setPixmap(button.grab())
        drag.exec(Qt.DropAction.MoveAction)
        self._refresh_later()  # hides empty stripes again (also after a cancelled drag)

    def _refresh_later(self, prefer: QDockWidget | None = None):
        # Rebuilding deletes the buttons, including the one being dragged, so
        # wait until the drag (and the mouse event that started it) is over.
        QTimer.singleShot(0, lambda: self.refresh(prefer=prefer))

    def move_to(self, name: str, area: Area, index: int):
        """Put the panel called name on the given edge, at position index on its stripe."""
        dock = next((d for d in self.docks if d.objectName() == name), None)
        if dock is None:
            return
        # Order: insert before the index-th other panel on that edge.
        others = [d for d in self.docks if d is not dock and self._area[d] == area]
        self.docks.remove(dock)
        if index < len(others):
            self.docks.insert(self.docks.index(others[index]), dock)
        else:
            self.docks.insert(self.docks.index(others[-1]) + 1 if others else len(self.docks), dock)
        if self._area[dock] != area:
            self._refreshing = True  # one refresh at the end, not one per signal
            try:
                self.window.addDockWidget(area, dock)
            finally:
                self._refreshing = False
            self._area[dock] = area
        dock.show()
        self._only(dock)
        self._refresh_later(prefer=dock)
