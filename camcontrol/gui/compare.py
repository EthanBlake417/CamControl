"""Side-by-side compare: a second image next to the main view.

The main view can be live, frozen, a file or a result; the compare pane
shows a saved image. With "Link" on, zooming or panning either one does
the same to the other, so you look at the same part of both. Images of
different sizes (e.g. 1920x1080 and 3264x1836) are matched by position in
the picture, not by pixel.

"Save both" writes both images into one file, with their names.
"""

from pathlib import Path

import cv2
import numpy as np
from PySide6.QtCore import Qt, Signal
from PySide6.QtWidgets import (
    QCheckBox,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QSizePolicy,
    QSplitter,
    QVBoxLayout,
    QWidget,
)

from camcontrol.gui.image_view import ImageView
from camcontrol.image_io import load_image_file
from camcontrol.processing.common import as_bgr

GAP_PX = 8  # white gap between the two images in a saved side-by-side


def side_by_side(left: np.ndarray, right: np.ndarray, left_name: str, right_name: str) -> np.ndarray:
    """Both images at the same height (the smaller one's), with names on top."""
    left, right = as_bgr(left), as_bgr(right)
    h = min(left.shape[0], right.shape[0])

    def scaled(im):
        if im.shape[0] == h:
            return im
        w = round(im.shape[1] * h / im.shape[0])
        return cv2.resize(im, (w, h), interpolation=cv2.INTER_AREA)

    left, right = scaled(left), scaled(right)
    gap = np.full((h, GAP_PX, 3), 255, np.uint8)
    out = np.hstack([left, gap, right])

    # Names in the top-left corner of each half, white on a dark box.
    scale = max(0.6, h / 1000)
    thick = max(1, round(scale * 2))
    for text, x0 in ((left_name, 0), (right_name, left.shape[1] + GAP_PX)):
        (tw, th), base = cv2.getTextSize(text, cv2.FONT_HERSHEY_SIMPLEX, scale, thick)
        pad = round(8 * scale)
        cv2.rectangle(out, (x0, 0), (x0 + tw + 2 * pad, th + base + 2 * pad), (0, 0, 0), -1)
        cv2.putText(out, text, (x0 + pad, th + pad), cv2.FONT_HERSHEY_SIMPLEX, scale,
                    (255, 255, 255), thick, cv2.LINE_AA)
    return out


def _title_label(text: str) -> QLabel:
    label = QLabel(text)
    label.setStyleSheet("font-weight: bold; padding: 2px 4px;")
    # Cut long names short instead of widening the pane.
    label.setSizePolicy(QSizePolicy.Policy.Ignored, QSizePolicy.Policy.Fixed)
    return label


class CompareArea(QWidget):
    """The window's centre: the main view, plus a compare view beside it.

    Not comparing: just the main view. Comparing: one button bar across the
    top, then the two images side by side, each under a title row of the same
    height, so both images get exactly the same space.
    """
    open_requested = Signal()   # "Open..." clicked
    save_requested = Signal()   # "Save both..." clicked
    close_requested = Signal()

    def __init__(self, main_view: ImageView, parent=None):
        super().__init__(parent)
        self.main_view = main_view
        self.view = ImageView()  # the compare image
        self.path: str | None = None
        self.comparing = False
        self._syncing = False

        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)

        # Button bar, shared by both halves.
        self.bar = QWidget()
        bar = QHBoxLayout(self.bar)
        bar.setContentsMargins(4, 2, 4, 2)
        bar.addWidget(QLabel("Compare:"))
        self.link_check = QCheckBox("Link zoom/pan")
        self.link_check.setToolTip("Zoom and pan both images together.")
        self.link_check.setChecked(True)
        self.link_check.toggled.connect(lambda on: on and self.sync_from(self.main_view))
        bar.addWidget(self.link_check)
        bar.addStretch()
        for text, tip, signal in (
            ("Open...", "Pick the image shown on the right.", self.open_requested),
            ("Save both...", "Save both images side by side in one file.", self.save_requested),
            ("Close", "Stop comparing (Ctrl+K).", self.close_requested),
        ):
            b = QPushButton(text)
            b.setToolTip(tip)
            b.clicked.connect(signal)
            bar.addWidget(b)
        layout.addWidget(self.bar)

        # The two halves: a title row over each image.
        self.main_title = _title_label("")
        self.compare_title = _title_label("Open an image, or right-click one in Captures")
        halves = []
        for title, view in ((self.main_title, main_view), (self.compare_title, self.view)):
            half = QWidget()
            v = QVBoxLayout(half)
            v.setContentsMargins(0, 0, 0, 0)
            v.setSpacing(0)
            v.addWidget(title)
            v.addWidget(view, stretch=1)
            halves.append(half)
        self.compare_half = halves[1]
        self.splitter = QSplitter(Qt.Orientation.Horizontal)
        for half in halves:
            self.splitter.addWidget(half)
        self.splitter.setChildrenCollapsible(False)
        layout.addWidget(self.splitter, stretch=1)

        # Follow each other's zoom and scroll position.
        for src in (main_view, self.view):
            src.zoom_changed.connect(lambda _z, s=src: self.sync_from(s))
            src.horizontalScrollBar().valueChanged.connect(lambda _v, s=src: self.sync_from(s))
            src.verticalScrollBar().valueChanged.connect(lambda _v, s=src: self.sync_from(s))

        self.set_comparing(False)

    def set_comparing(self, on: bool):
        was = self.comparing
        self.comparing = on
        for w in (self.bar, self.main_title, self.compare_half):
            w.setVisible(on)
        if on and not was:
            self.splitter.setSizes([10000, 10000])  # scaled to fit: two equal halves
            self.sync_from(self.main_view)

    def set_main_title(self, text: str, tooltip: str = ""):
        self.main_title.setText(text)
        self.main_title.setToolTip(tooltip)

    def load(self, path: str) -> bool:
        img = load_image_file(path)
        if img is None:
            return False
        self.path = path
        self.view.set_image(img)
        self.compare_title.setText(Path(path).name)
        self.compare_title.setToolTip(path)
        self.sync_from(self.main_view)
        return True

    def sync_from(self, src: ImageView):
        """Make the other view show the same part of its image as src."""
        if self._syncing or not self.comparing or not self.link_check.isChecked():
            return
        dst = self.view if src is self.main_view else self.main_view
        if src.image is None or dst.image is None:
            return
        sh, sw = src.image.shape[:2]
        dh, dw = dst.image.shape[:2]
        self._syncing = True
        try:
            if src._fit_mode:
                dst.fit()
            else:
                dst.set_zoom(src.zoom * sw / dw)  # same share of the picture on screen
            centre = src.mapToScene(src.viewport().rect().center())
            dst.centerOn(centre.x() / sw * dw, centre.y() / sh * dh)
        finally:
            self._syncing = False
