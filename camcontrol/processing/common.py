"""Helpers shared by the processing tools: color conversion, alignment."""

import cv2
import numpy as np

ALIGN_WIDTH = 1000  # alignment is worked out on a copy about this wide (faster, less noise)


def as_bgr(image: np.ndarray) -> np.ndarray:
    """Grayscale -> 3-channel BGR; BGR images are returned unchanged."""
    if image.ndim == 2:
        return cv2.cvtColor(image, cv2.COLOR_GRAY2BGR)
    return image


def as_gray(image: np.ndarray) -> np.ndarray:
    if image.ndim == 3:
        return cv2.cvtColor(image, cv2.COLOR_BGR2GRAY)
    return image


def check_same_size(images: list[np.ndarray]):
    sizes = {im.shape[:2] for im in images}
    if len(sizes) > 1:
        listed = ", ".join(f"{w}x{h}" for h, w in sorted(sizes))
        raise ValueError(f"All images must be the same size (got {listed}).")


def align_to(reference: np.ndarray, image: np.ndarray) -> np.ndarray:
    """Shift/rotate/scale image so it lines up with reference.

    Uses OpenCV's ECC method with an affine model, which covers the small
    moves and the slight change in magnification you get when refocusing.
    If alignment can't be found, the image is returned unchanged.
    """
    h, w = reference.shape[:2]
    scale = min(1.0, ALIGN_WIDTH / w)
    small = (max(1, round(w * scale)), max(1, round(h * scale)))

    def prep(im):
        g = cv2.resize(as_gray(im), small, interpolation=cv2.INTER_AREA)
        return g.astype(np.float32)

    warp = np.eye(2, 3, dtype=np.float32)
    criteria = (cv2.TERM_CRITERIA_EPS | cv2.TERM_CRITERIA_COUNT, 100, 1e-5)
    try:
        _, warp = cv2.findTransformECC(prep(reference), prep(image), warp,
                                       cv2.MOTION_AFFINE, criteria, None, 5)
    except cv2.error:
        return image  # didn't converge (e.g. very blurry or featureless)
    warp[:, 2] /= scale  # the shift was found on the small copy
    return cv2.warpAffine(image, warp, (w, h),
                          flags=cv2.INTER_LINEAR | cv2.WARP_INVERSE_MAP,
                          borderMode=cv2.BORDER_REPLICATE)


def align_all(images: list[np.ndarray], reference_index: int | None = None) -> list[np.ndarray]:
    """Align every image to one of them (the middle one by default)."""
    if reference_index is None:
        reference_index = len(images) // 2
    ref = images[reference_index]
    return [im if i == reference_index else align_to(ref, im) for i, im in enumerate(images)]
