"""numpy image -> Qt image conversion."""

import cv2
import numpy as np
from PySide6.QtGui import QImage, QPixmap


def to_pixmap(image: np.ndarray) -> QPixmap:
    """An 8-bit BGR or grayscale numpy image as a QPixmap, for showing (e.g.
    every live frame). Color goes through 32-bit BGRA, the pixmap's own
    layout, which converts about twice as fast as 24-bit; QPixmap.fromImage
    copies the data, so no extra copy is needed."""
    if image.ndim == 3:
        image = cv2.cvtColor(image, cv2.COLOR_BGR2BGRA)
        fmt = QImage.Format.Format_RGB32
    else:
        image = np.ascontiguousarray(image)
        fmt = QImage.Format.Format_Grayscale8
    h, w = image.shape[:2]
    return QPixmap.fromImage(QImage(image.data, w, h, image.strides[0], fmt))


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
