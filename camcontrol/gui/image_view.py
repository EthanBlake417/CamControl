"""Zoomable, pannable image display with overlays and measurement drawing.

Scene coordinates are image pixels: (0, 0) is the top-left corner of the
image, (w, h) the bottom-right. Measurement points are stored in these
coordinates (sub-pixel floats), so they stay put when zooming or panning.

Mouse, no tool selected:
    wheel         zoom around the cursor
    left drag     pan

Mouse and keys, with a measurement tool selected:
    left click    add a point (line/circle/angle/rectangle finish by themselves)
    double-click, right-click or Enter
                  finish a polyline / polygon
    Backspace     remove the last point
    Esc           cancel the shape being drawn (press again to leave the tool)
    middle drag   pan
    wheel         zoom
"""

import math

import numpy as np
from PySide6.QtCore import QPointF, QRectF, Qt, Signal
from PySide6.QtGui import QColor, QPainter, QPen, QPixmap
from PySide6.QtWidgets import QGraphicsPixmapItem, QGraphicsScene, QGraphicsView

from camcontrol.calibration import PIXELS, Calibration
from camcontrol.gui.measure_draw import (
    MEASURE_COLOR,
    PREVIEW_COLOR,
    SELECTED_COLOR,
    Style,
    draw_measurement,
)
from camcontrol.gui.qt_image import to_qimage
from camcontrol.measure import KINDS, Measurement

GRID_COLOR = QColor(255, 255, 0)
CROSSHAIR_COLOR = QColor(255, 0, 0)
CROSSHAIR_RADIUS_PX = 20  # on screen, whatever the zoom


class ImageView(QGraphicsView):
    cursor_moved = Signal(object)                 # (x, y, value) in image pixels, or None
    zoom_changed = Signal(float)                  # 1.0 = one image pixel per screen pixel
    measurement_drawn = Signal(str, list)         # kind, points (image pixels)
    tool_exit_requested = Signal()                # Esc with nothing being drawn

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

        # Measurements to draw (owned by the measurement panel).
        self.measurements: list[Measurement] = []
        self.calibration: Calibration = PIXELS
        self.selected_id: int | None = None

        # Shape being drawn right now.
        self.tool: str | None = None
        self._points: list[tuple[float, float]] = []
        self._mouse: tuple[float, float] | None = None
        self._pan_from: QPointF | None = None  # middle-button pan

        self.setDragMode(QGraphicsView.DragMode.ScrollHandDrag)
        self.setTransformationAnchor(QGraphicsView.ViewportAnchor.AnchorUnderMouse)
        self.setResizeAnchor(QGraphicsView.ViewportAnchor.AnchorViewCenter)
        # Redraw everything each time, so the overlays never leave trails.
        self.setViewportUpdateMode(QGraphicsView.ViewportUpdateMode.FullViewportUpdate)
        self.setBackgroundBrush(QColor(30, 30, 30))
        self.setMouseTracking(True)
        self.setFocusPolicy(Qt.FocusPolicy.StrongFocus)  # so Esc/Enter/Backspace reach us
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

    # --- overlays and measurements ------------------------------------------------

    def set_show_grid(self, on: bool):
        self.show_grid = on
        self.viewport().update()

    def set_show_crosshair(self, on: bool):
        self.show_crosshair = on
        self.viewport().update()

    def set_measurements(self, measurements, calibration, selected_id=None):
        self.measurements = measurements
        self.calibration = calibration
        self.selected_id = selected_id
        self.viewport().update()

    def set_selected(self, measurement_id):
        self.selected_id = measurement_id
        self.viewport().update()

    def drawForeground(self, painter: QPainter, rect: QRectF):
        """Draw overlays on top of the image."""
        if self._image is None:
            return
        h, w = self._image.shape[:2]

        # Grid and crosshair: drawn in image-pixel coordinates.
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

        # Measurements: drawn in window pixels, so lines and text keep the
        # same on-screen size at any zoom.
        painter.save()
        painter.resetTransform()
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        to_out = self.viewportTransform().map
        style = Style()
        for m in self.measurements:
            color = SELECTED_COLOR if m.id == self.selected_id else MEASURE_COLOR
            draw_measurement(painter, m, self.calibration, to_out, self.zoom, color, style)
        preview = self._preview()
        if preview is not None:
            draw_measurement(painter, preview, self.calibration, to_out, self.zoom, PREVIEW_COLOR, style)
        painter.restore()

    def _preview(self) -> Measurement | None:
        """The shape being drawn, including the point under the mouse."""
        if not self.tool or not self._points:
            return None
        points = list(self._points)
        n = KINDS[self.tool].n_points
        if self._mouse is not None and (n is None or len(points) < n):
            points.append(self._mouse)
        return Measurement(0, self.tool, points)

    # --- measurement tools ---------------------------------------------------------

    def set_tool(self, kind: str | None):
        """Select a measurement tool (a key of measure.KINDS), or None to pan."""
        self.tool = kind
        self._points = []
        if kind:
            self.setDragMode(QGraphicsView.DragMode.NoDrag)
            self.viewport().setCursor(Qt.CursorShape.CrossCursor)
        else:
            self.setDragMode(QGraphicsView.DragMode.ScrollHandDrag)
            self.viewport().unsetCursor()
        self.viewport().update()

    def _to_image(self, view_pos: QPointF) -> tuple[float, float]:
        """Window position -> image pixel coordinates (sub-pixel, clamped to the image)."""
        inverse, _ = self.viewportTransform().inverted()
        p = inverse.map(QPointF(view_pos))
        h, w = self._image.shape[:2]
        return min(max(p.x(), 0.0), float(w)), min(max(p.y(), 0.0), float(h))

    def _finish(self):
        kind = KINDS[self.tool]
        if len(self._points) >= kind.min_points:
            self.measurement_drawn.emit(self.tool, list(self._points))
        self._points = []
        self.viewport().update()

    # --- mouse and keys --------------------------------------------------------------

    def mousePressEvent(self, event):
        if event.button() == Qt.MouseButton.MiddleButton:
            self._pan_from = event.position()
            return
        if self.tool and self._image is not None:
            if event.button() == Qt.MouseButton.LeftButton:
                self._points.append(self._to_image(event.position()))
                if len(self._points) == KINDS[self.tool].n_points:
                    self._finish()
                self.viewport().update()
                return
            if event.button() == Qt.MouseButton.RightButton:
                self._finish()  # finishes an open shape; otherwise just clears it
                return
        super().mousePressEvent(event)

    def mouseDoubleClickEvent(self, event):
        # The first click of the double-click already added the point.
        if self.tool and KINDS[self.tool].n_points is None:
            self._finish()
            return
        super().mouseDoubleClickEvent(event)

    def mouseMoveEvent(self, event):
        if self._pan_from is not None:
            delta = event.position() - self._pan_from
            self._pan_from = event.position()
            self.horizontalScrollBar().setValue(self.horizontalScrollBar().value() - round(delta.x()))
            self.verticalScrollBar().setValue(self.verticalScrollBar().value() - round(delta.y()))
            return
        super().mouseMoveEvent(event)
        if self._image is not None:
            self._mouse = self._to_image(event.position())
            if self.tool:
                self.viewport().update()
        self._report_cursor(event.position())

    def mouseReleaseEvent(self, event):
        if event.button() == Qt.MouseButton.MiddleButton:
            self._pan_from = None
            return
        super().mouseReleaseEvent(event)

    def keyPressEvent(self, event):
        key = event.key()
        if self.tool:
            if key == Qt.Key.Key_Escape:
                if self._points:
                    self._points = []
                    self.viewport().update()
                else:
                    self.tool_exit_requested.emit()
                return
            if key in (Qt.Key.Key_Return, Qt.Key.Key_Enter):
                self._finish()
                return
            if key == Qt.Key.Key_Backspace and self._points:
                self._points.pop()
                self.viewport().update()
                return
        super().keyPressEvent(event)

    def wheelEvent(self, event):
        steps = event.angleDelta().y() / 120
        if steps:
            self.set_zoom(self.zoom * self.ZOOM_STEP ** steps)

    def resizeEvent(self, event):
        super().resizeEvent(event)
        if self._fit_mode:
            self.fit()

    def leaveEvent(self, event):
        super().leaveEvent(event)
        self._mouse = None
        self.cursor_moved.emit(None)
        self.viewport().update()

    def _report_cursor(self, view_pos: QPointF):
        if self._image is None:
            return
        inverse, _ = self.viewportTransform().inverted()
        p = inverse.map(QPointF(view_pos))
        x, y = math.floor(p.x()), math.floor(p.y())
        h, w = self._image.shape[:2]
        if 0 <= x < w and 0 <= y < h:
            px = self._image[y, x]
            # numpy stores BGR; report RGB, which is what people expect.
            value = int(px) if np.ndim(px) == 0 else tuple(int(v) for v in px[2::-1])
            self.cursor_moved.emit((x, y, value))
        else:
            self.cursor_moved.emit(None)
