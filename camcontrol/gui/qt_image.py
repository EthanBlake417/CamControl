"""numpy image -> Qt image conversion."""

import numpy as np
from PySide6.QtGui import QImage


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
