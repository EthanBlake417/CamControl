"""The window's centre: one or more views, each showing its own source (a
camera, an image file, a video or a processing result), in a layout.

- One view: just the image, as always.
- More: a bar across the top (layout, Link views, add a view, save all as
  one image) and a title row on each view, with its source menu (choose a
  camera or a file, live / freeze, remove the view).
- Layouts: Single (one view at a time, chosen with the tabs in the bar),
  Side by side, Stacked, Grid (the number of columns that shows the views
  largest in the space available).
- Link views: zooming or panning one view does the same to the others, so
  you look at the same part of each (images of different sizes are matched
  by position in the picture, not by pixel). The main window also makes
  capture, Live/Freeze, Fit and Find fiber axes act on every view then.
- The view clicked last is "active": the panels (Measurements, Counting,
  Fiber axis) work on it, and its title is highlighted.

"Save all as one image" writes all the images into one file, with their names.
"""

import math

import cv2
import numpy as np
from PySide6.QtCore import Qt, Signal
from PySide6.QtWidgets import (
    QButtonGroup,
    QCheckBox,
    QGridLayout,
    QHBoxLayout,
    QLabel,
    QMenu,
    QPushButton,
    QSizePolicy,
    QToolButton,
    QVBoxLayout,
    QWidget,
)

from camcontrol.gui.image_view import ImageView
from camcontrol.processing.common import as_bgr

GAP_PX = 8           # white gap between the images in a saved grid
MAX_VIEWS = 12
LAYOUTS = {  # name: label
    "single": "Single",
    "row": "Side by side",
    "column": "Stacked",
    "grid": "Grid",
}
TITLE_STYLE = "font-weight: bold; padding: 2px 4px;"
ACTIVE_STYLE = TITLE_STYLE + " background: #cad8ef;"


def best_columns(n: int, width: float, height: float, aspect: float = 16 / 9) -> int:
    """Columns that show n images of this aspect ratio largest in width x height."""
    best, best_size = 1, -1.0
    for cols in range(1, n + 1):
        rows = math.ceil(n / cols)
        cell_w, cell_h = width / cols, height / rows
        size = min(cell_w, cell_h * aspect)  # displayed image width
        if size > best_size + 1e-6:
            best, best_size = cols, size
    return best


def grid_image(images: list[np.ndarray], names: list[str]) -> np.ndarray:
    """The images in a grid (as square as possible), all at the smallest one's
    height, each with its name in the top-left corner."""
    images = [as_bgr(im) for im in images]
    h = min(im.shape[0] for im in images)

    def scaled(im):
        if im.shape[0] == h:
            return im
        w = round(im.shape[1] * h / im.shape[0])
        return cv2.resize(im, (w, h), interpolation=cv2.INTER_AREA)

    def labelled(im, text):
        im = im.copy()
        scale = max(0.6, h / 1000)
        thick = max(1, round(scale * 2))
        (tw, th), base = cv2.getTextSize(text, cv2.FONT_HERSHEY_SIMPLEX, scale, thick)
        pad = round(8 * scale)
        cv2.rectangle(im, (0, 0), (tw + 2 * pad, th + base + 2 * pad), (0, 0, 0), -1)
        cv2.putText(im, text, (pad, th + pad), cv2.FONT_HERSHEY_SIMPLEX, scale, (255, 255, 255), thick, cv2.LINE_AA)
        return im

    tiles = [labelled(scaled(im), name) for im, name in zip(images, names)]
    cols = math.ceil(math.sqrt(len(tiles)))
    rows = []
    for r in range(0, len(tiles), cols):
        row = []
        for tile in tiles[r:r + cols]:
            if row:
                row.append(np.full((h, GAP_PX, 3), 255, np.uint8))
            row.append(tile)
        rows.append(np.hstack(row))
    width = max(r.shape[1] for r in rows)
    out = []
    for row in rows:
        if out:
            out.append(np.full((GAP_PX, width, 3), 255, np.uint8))
        if row.shape[1] < width:  # a short last row: pad on the right
            row = np.hstack([row, np.full((h, width - row.shape[1], 3), 255, np.uint8)])
        out.append(row)
    return np.vstack(out)


class _Cell(QWidget):
    """One view: a title row (what it shows, its source menu, x to remove) over the image."""

    def __init__(self, area: "ViewsArea", view: ImageView):
        super().__init__()
        self.view = view
        v = QVBoxLayout(self)
        v.setContentsMargins(0, 0, 0, 0)
        v.setSpacing(0)
        self.header = QWidget()
        h = QHBoxLayout(self.header)
        h.setContentsMargins(0, 0, 2, 0)
        h.setSpacing(0)
        self.title = QLabel("")
        self.title.setStyleSheet(TITLE_STYLE)
        # Cut long names short instead of widening the view.
        self.title.setSizePolicy(QSizePolicy.Policy.Ignored, QSizePolicy.Policy.Fixed)
        h.addWidget(self.title, stretch=1)
        source = QToolButton()
        source.setText("Source ")
        source.setToolTip("What this view shows: a camera or a file. Also Live/Freeze and Remove.")
        source.setPopupMode(QToolButton.ToolButtonPopupMode.InstantPopup)
        source.setAutoRaise(True)
        menu = QMenu(source)
        menu.aboutToShow.connect(lambda: area.source_menu_requested.emit(view, menu))
        source.setMenu(menu)
        h.addWidget(source)
        close = QToolButton()
        close.setText("×")
        close.setAutoRaise(True)
        close.setStyleSheet("font-size: 15px; font-weight: bold; padding: 0 4px;")
        close.setToolTip("Remove this view.")
        close.clicked.connect(lambda: area.remove_requested.emit(view))
        h.addWidget(close)
        v.addWidget(self.header)
        v.addWidget(view, stretch=1)
        # Every view gets the same share of the space, whatever it asks for.
        self.setSizePolicy(QSizePolicy.Policy.Ignored, QSizePolicy.Policy.Ignored)

    def set_active(self, on: bool):
        self.title.setStyleSheet(ACTIVE_STYLE if on else TITLE_STYLE)


class ViewsArea(QWidget):
    add_requested = Signal()                        # "+ Add view" clicked
    save_requested = Signal()                       # "Save all as one image..." clicked
    remove_requested = Signal(object)               # a view's x clicked: its ImageView
    source_menu_requested = Signal(object, object)  # a view's source menu is opening: ImageView, QMenu to fill
    layout_changed = Signal(str)                    # see LAYOUTS
    linked_changed = Signal(bool)
    tab_clicked = Signal(object)                    # Single layout: a view's tab clicked: its ImageView

    def __init__(self, parent=None):
        super().__init__(parent)
        self.layout_name = "single"
        self.multi_layout = "grid"  # the layout Ctrl+K goes back to
        self.active: ImageView | None = None
        self._syncing = False
        self._cols = None

        outer = QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        outer.setSpacing(0)

        # The bar, shown with two or more views.
        self.bar = QWidget()
        bar = QHBoxLayout(self.bar)
        bar.setContentsMargins(4, 2, 4, 2)
        bar.addWidget(QLabel("Layout:"))
        self.layout_buttons = QButtonGroup(self)
        for name, label in LAYOUTS.items():
            b = QToolButton()
            b.setText(label)
            b.setCheckable(True)
            b.setAutoRaise(True)
            b.clicked.connect(lambda _=False, n=name: self.set_layout(n))
            self.layout_buttons.addButton(b)
            b.setProperty("layout", name)
            bar.addWidget(b)
        bar.addSpacing(8)
        # Single layout: one tab per view.
        self.tabs = QWidget()
        self.tabs_layout = QHBoxLayout(self.tabs)
        self.tabs_layout.setContentsMargins(0, 0, 0, 0)
        self.tab_group = QButtonGroup(self)
        bar.addWidget(self.tabs)
        bar.addStretch()
        self.link_check = QCheckBox("Link views")
        self.link_check.setToolTip("Zoom and pan the views together, and make Capture, Live/Freeze, Fit, 100%\n"
                                   "and Find fiber axes act on all of them. Off: they act on the active view\n"
                                   "(the one clicked last, with the highlighted title).")
        self.link_check.setChecked(True)
        self.link_check.toggled.connect(self._link_toggled)
        bar.addWidget(self.link_check)
        self.add_button = QPushButton("+ Add view")
        self.add_button.setToolTip("Add a view; choose its camera or image with its Source menu.")
        self.add_button.clicked.connect(self.add_requested)
        bar.addWidget(self.add_button)
        save = QPushButton("Save all as one image...")
        save.setToolTip("Save the images of all the views in one file, with their names.")
        save.clicked.connect(self.save_requested)
        bar.addWidget(save)
        outer.addWidget(self.bar)

        self.grid_widget = QWidget()
        self.grid = QGridLayout(self.grid_widget)
        self.grid.setContentsMargins(0, 0, 0, 0)
        self.grid.setSpacing(4)
        outer.addWidget(self.grid_widget, stretch=1)
        self.cells: list[_Cell] = []
        self._update_bar()

    # --- views ------------------------------------------------------------------------

    @property
    def views(self) -> list[ImageView]:
        return [c.view for c in self.cells]

    @property
    def linked(self) -> bool:
        return len(self.cells) > 1 and self.link_check.isChecked()

    def _cell(self, view: ImageView) -> _Cell:
        return next(c for c in self.cells if c.view is view)

    def is_full(self) -> bool:
        return len(self.cells) >= MAX_VIEWS

    def add_view(self, view: ImageView, title: str = "", tooltip: str = ""):
        cell = _Cell(self, view)
        self.cells.append(cell)
        self.set_title(view, title, tooltip)
        view.zoom_changed.connect(lambda _z, s=view: self.sync_from(s))
        view.horizontalScrollBar().valueChanged.connect(lambda _v, s=view: self.sync_from(s))
        view.verticalScrollBar().valueChanged.connect(lambda _v, s=view: self.sync_from(s))
        if self.active is None:
            self.active = view
        self._update_bar()
        self._relayout(force=True)
        if len(self.cells) > 1:
            self.sync_from(self.active)

    def remove_view(self, view: ImageView):
        cell = self._cell(view)
        self.cells.remove(cell)
        self.grid.removeWidget(cell)
        cell.setParent(None)
        cell.deleteLater()  # deletes the view with it
        if len(self.cells) == 1:
            self.set_layout("single")
        self._update_bar()
        self._relayout(force=True)

    def set_title(self, view: ImageView, text: str, tooltip: str = ""):
        cell = self._cell(view)
        number = self.cells.index(cell) + 1
        cell.title.setText(f"{number}  {text}")
        cell.title.setToolTip(tooltip or "Click a view to work on it (measure, count, fiber axes, Controls).")
        self._update_tabs()

    def set_active(self, view: ImageView):
        """Highlight the title of the view the panels work on (in Single layout, show it)."""
        self.active = view
        for c in self.cells:
            c.set_active(c.view is view)
        self._update_tabs()
        if self.layout_name == "single":
            self._relayout(force=True)

    def set_layout(self, name: str):
        if name not in LAYOUTS:
            return
        if name != "single":
            self.multi_layout = name
        changed = name != self.layout_name
        self.layout_name = name
        for b in self.layout_buttons.buttons():
            b.setChecked(b.property("layout") == name)
        self._update_bar()
        self._relayout(force=True)
        if self.active is not None:
            self.sync_from(self.active)
        if changed:
            self.layout_changed.emit(name)

    def set_linked(self, on: bool):
        self.link_check.setChecked(on)

    def _link_toggled(self, on: bool):
        if on and self.active is not None:
            self.sync_from(self.active)
        self.linked_changed.emit(on)

    # --- bar and layout -----------------------------------------------------------------

    def _update_bar(self):
        several = len(self.cells) > 1
        self.bar.setVisible(several)
        self.tabs.setVisible(self.layout_name == "single")
        self.add_button.setEnabled(not self.is_full())
        for c in self.cells:  # titles show with several views, except in Single (the tabs name them)
            c.header.setVisible(several and self.layout_name != "single")
        for b in self.layout_buttons.buttons():
            b.setChecked(b.property("layout") == self.layout_name)
        self._update_tabs()

    def _update_tabs(self):
        for b in self.tab_group.buttons():
            self.tab_group.removeButton(b)
        while self.tabs_layout.count():  # out of the bar now, not just when deleted later
            old = self.tabs_layout.takeAt(0).widget()
            if old is not None:
                old.setParent(None)
                old.deleteLater()
        if self.layout_name != "single" or len(self.cells) < 2:
            return
        for i, c in enumerate(self.cells):
            b = QToolButton()
            b.setText(c.title.text() or str(i + 1))
            b.setToolTip(c.title.toolTip())
            b.setCheckable(True)
            b.setChecked(c.view is self.active)
            b.setAutoRaise(True)
            b.setMaximumWidth(220)
            b.clicked.connect(lambda _=False, v=c.view: self.tab_clicked.emit(v))
            self.tab_group.addButton(b)
            self.tabs_layout.addWidget(b)

    def _shown(self) -> list[_Cell]:
        if self.layout_name == "single":
            return [c for c in self.cells if c.view is self.active][:1] or self.cells[:1]
        return list(self.cells)

    def _columns(self, n: int) -> int:
        if self.layout_name == "row":
            return n
        if self.layout_name == "column":
            return 1
        size = self.grid_widget.size()
        return best_columns(n, max(size.width(), 1), max(size.height(), 1))

    def _relayout(self, force: bool = False):
        shown = self._shown()
        cols = self._columns(len(shown))
        if not force and cols == self._cols:
            return
        self._cols = cols
        for i in reversed(range(self.grid.count())):
            self.grid.takeAt(i)
        for c in range(self.grid.columnCount()):
            self.grid.setColumnStretch(c, 0)
        for r in range(self.grid.rowCount()):
            self.grid.setRowStretch(r, 0)
        for c in self.cells:
            c.setVisible(c in shown)
        for i, w in enumerate(shown):
            self.grid.addWidget(w, i // cols, i % cols)
        for c in range(cols):
            self.grid.setColumnStretch(c, 1)
        for r in range(math.ceil(len(shown) / cols)):
            self.grid.setRowStretch(r, 1)

    def resizeEvent(self, event):
        super().resizeEvent(event)
        self._relayout()

    # --- linked zoom ------------------------------------------------------------------

    def sync_from(self, src: ImageView):
        """Make the other views show the same part of their images as src."""
        if self._syncing or not self.linked or src is None or src.image is None:
            return
        sh, sw = src.image.shape[:2]
        centre = src.mapToScene(src.viewport().rect().center())
        self._syncing = True
        try:
            for dst in self.views:
                if dst is src or dst.image is None:
                    continue
                dh, dw = dst.image.shape[:2]
                if src._fit_mode:
                    dst.fit()
                else:
                    dst.set_zoom(src.zoom * sw / dw)  # same share of the picture on screen
                dst.centerOn(centre.x() / sw * dw, centre.y() / sh * dh)
        finally:
            self._syncing = False
