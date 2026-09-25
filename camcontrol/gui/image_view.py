"""Zoomable, pannable image display with grid and crosshair overlays.

Built on QGraphicsView, so later the measurement tools can be added as
graphics items on the same scene. Scene coordinates are image pixels:
(0, 0) is the top-left corner of the image, (w, h) the bottom-right.

Mouse:
    wheel        zoom around the cursor
    left drag    pan
"""

import math

import numpy as np
from PySide6.QtCore import QPointF, QRectF, Qt, Signal
from PySide6.QtGui import QColor, QImage, QPainter, QPen, QPixmap
from PySide6.QtWidgets import QGraphicsPixmapItem, QGraphicsScene, QGraphicsView

GRID_COLOR = QColor(255, 255, 0)
CROSSHAIR_COLOR = QColor(255, 0, 0)
CROSSHAIR_RADIUS_PX = 20  # on screen, whatever the zoom


def to_qimage(image: np.ndarray) -> QImage:
    """Convert an 8-bit BGR or grayscale numpy image to a QImage (a copy)."""
    image = np.ascontiguousarray(image)
    h, w = image.shape[:2]
    if image.ndim == 2:
        fmt = QImage.Format.Format_Grayscale8
    else:
        fmt = QImage.Format.Format_BGR888
    # .copy() so the QImage owns its data and doesn't point into numpy memory.
    return QImage(image.data, w, h, image.strides[0], fmt).copy()


class ImageView(QGraphicsView):
    cursor_moved = Signal(object)  # (x, y, value) in image pixels, or None
    zoom_changed = Signal(float)   # 1.0 = one image pixel per screen pixel

    ZOOM_STEP = 1.25
    MIN_ZOOM = 0.05
    MAX_ZOOM = 32.0
    PIXELATED_ZOOM = 4.0  # at or above this, show pixels as sharp blocks

    def __init__(self, parent=None):
        super().__init__(parent)
        self._scene = QGraphicsScene(self)
        self.setScene(self._scene)
        self._pixmap_item = QGraphicsPixmapItem()
        self._scene.addItem(self._pixmap_item)

        self._image: np.ndarray | None = None
        self._fit_mode = True  # keep refitting on window resize until the user zooms
        self.show_grid = False
        self.show_crosshair = False
        self.grid_divisions = 8

        self.setDragMode(QGraphicsView.DragMode.ScrollHandDrag)
        self.setTransformationAnchor(QGraphicsView.ViewportAnchor.AnchorUnderMouse)
        self.setResizeAnchor(QGraphicsView.ViewportAnchor.AnchorViewCenter)
        # Redraw everything each time, so the overlays never leave trails.
        self.setViewportUpdateMode(QGraphicsView.ViewportUpdateMode.FullViewportUpdate)
        self.setBackgroundBrush(QColor(30, 30, 30))
        self.setMouseTracking(True)
        self._update_smoothing()

    # --- image -----------------------------------------------------------------

    @property
    def image(self) -> np.ndarray | None:
        return self._image

    def set_image(self, image: np.ndarray):
        new_size = self._image is None or image.shape[:2] != self._image.shape[:2]
        self._image = image
        self._pixmap_item.setPixmap(QPixmap.fromImage(to_qimage(image)))
        if new_size:
            self._scene.setSceneRect(QRectF(self._pixmap_item.boundingRect()))
            self.fit()

    # --- zoom ------------------------------------------------------------------

    @property
    def zoom(self) -> float:
        return self.transform().m11()

    def set_zoom(self, zoom: float):
        zoom = min(max(zoom, self.MIN_ZOOM), self.MAX_ZOOM)
        factor = zoom / self.zoom
        self.scale(factor, factor)
        self._fit_mode = False
        self._zoom_updated()

    def zoom_in(self):
        self.set_zoom(self.zoom * self.ZOOM_STEP)

    def zoom_out(self):
        self.set_zoom(self.zoom / self.ZOOM_STEP)

    def actual_size(self):
        """One image pixel per screen pixel."""
        self.set_zoom(1.0)

    def fit(self):
        """Fit the whole image in the window."""
        if self._image is None:
            return
        self.fitInView(self._pixmap_item, Qt.AspectRatioMode.KeepAspectRatio)
        self._fit_mode = True
        self._zoom_updated()

    def _zoom_updated(self):
        self._update_smoothing()
        self.viewport().update()
        self.zoom_changed.emit(self.zoom)

    def _update_smoothing(self):
        # Smooth when zoomed out (looks better); blocky when zoomed far in
        # (so you can see individual pixels, which matters for measuring).
        smooth = self.zoom < self.PIXELATED_ZOOM
        self._pixmap_item.setTransformationMode(
            Qt.TransformationMode.SmoothTransformation if smooth
            else Qt.TransformationMode.FastTransformation
        )

    # --- overlays ----------------------------------------------------------------

    def set_show_grid(self, on: bool):
        self.show_grid = on
        self.viewport().update()

    def set_show_crosshair(self, on: bool):
        self.show_crosshair = on
        self.viewport().update()

    def drawForeground(self, painter: QPainter, rect: QRectF):
        """Draw overlays on top of the image, in image-pixel coordinates."""
        if self._image is None:
            return
        h, w = self._image.shape[:2]

        if self.show_grid:
            pen = QPen(GRID_COLOR, 1)
            pen.setCosmetic(True)  # 1 screen pixel wide at any zoom
            painter.setPen(pen)
            n = self.grid_divisions
            for i in range(1, n):
                x = w * i / n
                y = h * i / n
                painter.drawLine(QPointF(x, 0), QPointF(x, h))
                painter.drawLine(QPointF(0, y), QPointF(w, y))

        if self.show_crosshair:
            pen = QPen(CROSSHAIR_COLOR, 2)
            pen.setCosmetic(True)
            painter.setPen(pen)
            painter.setBrush(Qt.BrushStyle.NoBrush)
            cx, cy = w / 2, h / 2
            painter.drawLine(QPointF(cx, 0), QPointF(cx, h))
            painter.drawLine(QPointF(0, cy), QPointF(w, cy))
            r = CROSSHAIR_RADIUS_PX / self.zoom
            painter.drawEllipse(QPointF(cx, cy), r, r)

    # --- mouse ---------------------------------------------------------------------

    def wheelEvent(self, event):
        steps = event.angleDelta().y() / 120
        if steps:
            self.set_zoom(self.zoom * self.ZOOM_STEP ** steps)

    def resizeEvent(self, event):
        super().resizeEvent(event)
        if self._fit_mode:
            self.fit()

    def mouseMoveEvent(self, event):
        super().mouseMoveEvent(event)
        self._report_cursor(event.position().toPoint())

    def leaveEvent(self, event):
        super().leaveEvent(event)
        self.cursor_moved.emit(None)

    def _report_cursor(self, view_pos):
        if self._image is None:
            return
        p = self.mapToScene(view_pos)
        x, y = math.floor(p.x()), math.floor(p.y())
        h, w = self._image.shape[:2]
        if 0 <= x < w and 0 <= y < h:
            px = self._image[y, x]
            # numpy stores BGR; report RGB, which is what people expect.
            value = int(px) if np.ndim(px) == 0 else tuple(int(v) for v in px[2::-1])
            self.cursor_moved.emit((x, y, value))
        else:
            self.cursor_moved.emit(None)
