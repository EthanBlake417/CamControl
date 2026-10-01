"""Compare grid: the main view plus any number of saved images, in a grid.

The main view (live, frozen, a file or a result) is always the first cell.
Images are added with "Add images..." or from Captures (right-click), and
each has a x button to remove it. The grid picks the number of columns that
shows the images largest in the space available. With "Link" on, zooming or
panning any image does the same to all the others, so you look at the same
part of each. Images of different sizes (e.g. 1920x1080 and 3264x1836) are
matched by position in the picture, not by pixel.

"Save grid" writes all the images into one file, with their names.
"""

import math

import cv2
import numpy as np
from PySide6.QtCore import Qt, Signal
from PySide6.QtWidgets import (
    QCheckBox,
    QGridLayout,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QSizePolicy,
    QToolButton,
    QVBoxLayout,
    QWidget,
)

from camcontrol.gui.image_view import ImageView
from camcontrol.processing.common import as_bgr

GAP_PX = 8           # white gap between the images in a saved grid
MAX_IMAGES = 12      # cells in the grid, the main view included
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
    """One image of the grid: a title row (name, and x to remove) over the view."""

    def __init__(self, view: ImageView, removable: bool, on_remove):
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
        # Cut long names short instead of widening the cell.
        self.title.setSizePolicy(QSizePolicy.Policy.Ignored, QSizePolicy.Policy.Fixed)
        h.addWidget(self.title, stretch=1)
        close = QToolButton()
        close.setText("×")
        close.setAutoRaise(True)
        close.setStyleSheet("font-size: 15px; font-weight: bold; padding: 0 4px;")
        close.setToolTip("Remove this image from the grid.")
        close.clicked.connect(lambda: on_remove(view))
        h.addWidget(close)
        if not removable:
            # Hidden but still taking its space, so every title row (and image) is the same size.
            policy = close.sizePolicy()
            policy.setRetainSizeWhenHidden(True)
            close.setSizePolicy(policy)
            close.hide()
        v.addWidget(self.header)
        v.addWidget(view, stretch=1)
        # Every cell gets the same share of the grid, whatever the view asks for.
        self.setSizePolicy(QSizePolicy.Policy.Ignored, QSizePolicy.Policy.Ignored)

    def set_active(self, on: bool):
        self.title.setStyleSheet(ACTIVE_STYLE if on else TITLE_STYLE)


class CompareArea(QWidget):
    """The window's centre: the main view, plus the compare images in a grid.

    Not comparing: just the main view. Comparing: one button bar across the
    top, then a grid of equal cells, each with its image's name above it.
    """
    add_requested = Signal()       # "Add images..." clicked
    save_requested = Signal()      # "Save grid..." clicked
    close_requested = Signal()
    remove_requested = Signal(object)  # an image's x clicked: its ImageView
    remove_all_requested = Signal()

    def __init__(self, main_view: ImageView, parent=None):
        super().__init__(parent)
        self.main_view = main_view
        self.comparing = False
        self._syncing = False
        self._cols = 0

        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)

        # Button bar, shared by all the images.
        self.bar = QWidget()
        bar = QHBoxLayout(self.bar)
        bar.setContentsMargins(4, 2, 4, 2)
        bar.addWidget(QLabel("Compare:"))
        self.link_check = QCheckBox("Link zoom/pan")
        self.link_check.setToolTip("Zoom and pan all the images together.")
        self.link_check.setChecked(True)
        self.link_check.toggled.connect(lambda on: on and self.sync_from(self.main_view))
        bar.addWidget(self.link_check)
        bar.addStretch()
        self.count_label = QLabel("")
        self.count_label.setStyleSheet("color: gray;")
        bar.addWidget(self.count_label)
        self.add_button = QPushButton("Add images...")
        for button, tip, signal in (
            (self.add_button, "Add saved images to the grid (or right-click images in Captures).", self.add_requested),
            (QPushButton("Remove all"), "Remove all the added images (the main view stays).", self.remove_all_requested),
            (QPushButton("Save grid..."), "Save all the images in one file, with their names.", self.save_requested),
            (QPushButton("Close"), "Stop comparing (Ctrl+K). The added images stay for next time.", self.close_requested),
        ):
            button.setToolTip(tip)
            button.clicked.connect(signal)
            bar.addWidget(button)
        layout.addWidget(self.bar)

        self.grid_widget = QWidget()
        self.grid = QGridLayout(self.grid_widget)
        self.grid.setContentsMargins(0, 0, 0, 0)
        self.grid.setSpacing(4)
        layout.addWidget(self.grid_widget, stretch=1)

        self.placeholder = QLabel("Add images with the button above,\nor right-click images in Captures.")
        self.placeholder.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.placeholder.setStyleSheet("color: gray; border: 1px dashed #aaa;")
        self.placeholder.setSizePolicy(QSizePolicy.Policy.Ignored, QSizePolicy.Policy.Ignored)

        self.cells: list[_Cell] = []
        self.add_view(main_view, removable=False)
        self.set_comparing(False)

    # --- cells ------------------------------------------------------------------------

    @property
    def views(self) -> list[ImageView]:
        return [c.view for c in self.cells]

    def _cell(self, view: ImageView) -> _Cell:
        return next(c for c in self.cells if c.view is view)

    def is_full(self) -> bool:
        return len(self.cells) >= MAX_IMAGES

    def add_view(self, view: ImageView, title: str = "", tooltip: str = "", removable: bool = True):
        cell = _Cell(view, removable, self.remove_requested.emit)
        self.cells.append(cell)
        self.set_title(view, title, tooltip)
        view.zoom_changed.connect(lambda _z, s=view: self.sync_from(s))
        view.horizontalScrollBar().valueChanged.connect(lambda _v, s=view: self.sync_from(s))
        view.verticalScrollBar().valueChanged.connect(lambda _v, s=view: self.sync_from(s))
        self._relayout(force=True)
        if len(self.cells) > 1:
            self.sync_from(self.main_view)

    def remove_view(self, view: ImageView):
        cell = self._cell(view)
        self.cells.remove(cell)
        self.grid.removeWidget(cell)
        cell.setParent(None)
        cell.deleteLater()  # deletes the view with it
        self._relayout(force=True)

    def set_title(self, view: ImageView, text: str, tooltip: str = ""):
        cell = self._cell(view)
        cell.title.setText(text)
        cell.title.setToolTip(tooltip or "Click an image to measure, count or find fiber axes on it.")

    def set_active(self, view: ImageView):
        """Highlight the title of the image the panels work on."""
        for c in self.cells:
            c.set_active(c.view is view)

    def set_comparing(self, on: bool):
        was = self.comparing
        self.comparing = on
        self.bar.setVisible(on)
        self._relayout(force=True)
        if on and not was:
            self.sync_from(self.main_view)

    # --- layout -----------------------------------------------------------------------

    def _shown(self) -> list[QWidget]:
        if not self.comparing:
            return [self.cells[0]]
        return list(self.cells) if len(self.cells) > 1 else [self.cells[0], self.placeholder]

    def _relayout(self, force: bool = False):
        shown = self._shown()
        size = self.grid_widget.size()
        cols = best_columns(len(shown), max(size.width(), 1), max(size.height(), 1))
        if not force and cols == self._cols:
            return
        self._cols = cols
        for i in reversed(range(self.grid.count())):
            self.grid.takeAt(i)
        for c in range(self.grid.columnCount()):
            self.grid.setColumnStretch(c, 0)
        for r in range(self.grid.rowCount()):
            self.grid.setRowStretch(r, 0)
        for w in [*self.cells, self.placeholder]:
            w.setVisible(w in shown)
        for i, w in enumerate(shown):
            self.grid.addWidget(w, i // cols, i % cols)
        for c in range(cols):
            self.grid.setColumnStretch(c, 1)
        for r in range(math.ceil(len(shown) / cols)):
            self.grid.setRowStretch(r, 1)
        for c in self.cells:  # only the grid shows titles
            c.header.setVisible(self.comparing)
        self.count_label.setText(f"{len(self.cells) - 1} added")
        self.add_button.setEnabled(not self.is_full())

    def resizeEvent(self, event):
        super().resizeEvent(event)
        self._relayout()

    # --- linked zoom ------------------------------------------------------------------

    def sync_from(self, src: ImageView):
        """Make the other views show the same part of their images as src."""
        if self._syncing or not self.comparing or not self.link_check.isChecked() or src.image is None:
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
