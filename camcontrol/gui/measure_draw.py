"""Drawing measurements with QPainter, used both on screen and for export.

Everything is drawn in "output" coordinates through a to_out() function
that maps image pixels to wherever we're drawing:
  - on screen: image pixels -> window pixels (depends on zoom and pan)
  - annotated export: image pixels -> the same image pixels
So one function draws both, and line widths / text sizes stay readable in
each case.
"""

from dataclasses import dataclass

import numpy as np
from PySide6.QtCore import QPointF, QRectF, Qt
from PySide6.QtGui import QColor, QFontMetricsF, QImage, QPainter, QPen, QPolygonF

from camcontrol.calibration import Calibration
from camcontrol.gui.qt_image import to_qimage
from camcontrol.measure import CIRCLE_KINDS, Measurement, compute, summary

MEASURE_COLOR = QColor(0, 255, 0)
SELECTED_COLOR = QColor(255, 140, 0)
PREVIEW_COLOR = QColor(0, 200, 255)


@dataclass
class Style:
    line_width: float = 2
    font_px: int = 14
    marker: float = 3  # half-size of the square drawn at each clicked point


def draw_measurement(painter: QPainter, m: Measurement, cal: Calibration, to_out, zoom: float,
                     color: QColor, style: Style, label: bool = True):
    pts = [to_out(QPointF(x, y)) for x, y in m.points]
    if not pts:
        return
    try:
        results = compute(m, cal)
    except ValueError:
        results = None  # incomplete or invalid (e.g. while still clicking)

    pen = QPen(color, style.line_width)
    painter.setPen(pen)
    painter.setBrush(Qt.BrushStyle.NoBrush)

    anchor = pts[0]
    if m.kind in ("line", "polyline", "angle"):
        if len(pts) >= 2:
            painter.drawPolyline(QPolygonF(pts))
        if m.kind == "line" and len(pts) >= 2:
            anchor = (pts[0] + pts[1]) / 2
        elif m.kind == "angle" and len(pts) >= 2:
            anchor = pts[1]
    elif m.kind == "polygon":
        if len(pts) >= 3:
            painter.drawPolygon(QPolygonF(pts))
        elif len(pts) == 2:
            painter.drawPolyline(QPolygonF(pts))
    elif m.kind == "rectangle" and len(pts) == 2:
        (x0, y0), (x1, y1) = m.points
        corners = [to_out(QPointF(x, y)) for x, y in ((x0, y0), (x1, y0), (x1, y1), (x0, y1))]
        painter.drawPolygon(QPolygonF(corners))
        anchor = QPointF(min(c.x() for c in corners), min(c.y() for c in corners))
    elif m.kind in CIRCLE_KINDS:
        if m.kind == "circle_centre" and len(pts) == 2:
            painter.drawLine(pts[0], pts[1])  # the radius being drawn out
        if results is not None:
            cx, cy = results["center_px"]
            centre = to_out(QPointF(cx, cy))
            # With non-square pixels the circle is an ellipse in pixels.
            rx = results["radius"] / cal.x * zoom
            ry = results["radius"] / cal.y * zoom
            painter.drawEllipse(centre, rx, ry)
            k = style.marker * 2
            painter.drawLine(centre - QPointF(k, 0), centre + QPointF(k, 0))
            painter.drawLine(centre - QPointF(0, k), centre + QPointF(0, k))
            anchor = centre
        elif len(pts) >= 2:
            painter.drawPolyline(QPolygonF(pts))

    k = style.marker
    for p in pts:
        painter.drawRect(QRectF(p.x() - k, p.y() - k, 2 * k, 2 * k))

    if label and results is not None:
        text = summary(m, results, cal)
        if m.id:
            text = f"{m.id}: {text}"
        draw_label(painter, anchor, text, color, style)


def draw_label(painter: QPainter, pos: QPointF, text: str, color: QColor, style: Style):
    """Text in a dark box just below-right of pos, readable on any image."""
    font = painter.font()
    font.setPixelSize(style.font_px)
    painter.setFont(font)
    fm = QFontMetricsF(font)
    pad = style.font_px * 0.25
    offset = style.font_px * 0.5
    rect = QRectF(pos.x() + offset, pos.y() + offset,
                  fm.horizontalAdvance(text) + 2 * pad, fm.height() + 2 * pad)
    painter.fillRect(rect, QColor(0, 0, 0, 170))
    painter.setPen(color)
    painter.drawText(rect, Qt.AlignmentFlag.AlignCenter, text)


def render_annotated(image: np.ndarray, measurements: list[Measurement], cal: Calibration) -> QImage:
    """A full-resolution copy of the image with the measurements drawn on it."""
    qimg = to_qimage(image).convertToFormat(QImage.Format.Format_RGB32)
    w = image.shape[1]
    # Scale line and text sizes with the image so they're readable when viewed.
    style = Style(line_width=max(2.0, w / 800), font_px=max(14, round(w / 70)), marker=max(3.0, w / 500))
    painter = QPainter(qimg)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    for m in measurements:
        draw_measurement(painter, m, cal, lambda q: q, 1.0, MEASURE_COLOR, style)
    painter.end()
    return qimg
