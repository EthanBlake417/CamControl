"""Reading and writing image files."""

from pathlib import Path

import cv2
import numpy as np

IMAGE_EXTENSIONS = {".tif", ".tiff", ".png", ".jpg", ".jpeg", ".bmp"}


def load_image_file(path) -> np.ndarray | None:
    """Read an image as 8-bit BGR or grayscale. Returns None if unreadable.

    Uses imdecode on the raw bytes, because cv2.imread can't open paths
    with non-ASCII characters on Windows.
    """
    try:
        data = np.fromfile(str(path), dtype=np.uint8)
    except OSError:
        return None
    img = cv2.imdecode(data, cv2.IMREAD_UNCHANGED)
    if img is None:
        return None
    if img.dtype == np.uint16:
        img = (img >> 8).astype(np.uint8)  # 16-bit -> 8-bit for display
    elif img.dtype != np.uint8:
        img = cv2.normalize(img, None, 0, 255, cv2.NORM_MINMAX).astype(np.uint8)
    if img.ndim == 3 and img.shape[2] == 4:
        img = cv2.cvtColor(img, cv2.COLOR_BGRA2BGR)
    return img


def save_image_file(path, image: np.ndarray):
    """Write an image; the format comes from the file extension.

    imencode + tofile instead of cv2.imwrite, which fails on Windows when
    the path has non-ASCII characters (e.g. "µ" in a name).
    """
    ext = Path(path).suffix or ".tif"
    ok, data = cv2.imencode(ext, image)
    if not ok:
        raise RuntimeError(f"Could not encode image as {ext}")
    data.tofile(str(path))
