"""Focus stacking (extended depth of field).

A MacroZoom lens has a shallow depth of field, so only a thin slice of a
3-D sample is sharp in one image. Take several images while stepping the
focus through the sample, and this combines the sharp parts of each:

1. Align the images (refocusing shifts and slightly scales the image).
2. For each image, measure sharpness at every pixel: the strength of the
   Laplacian (fine detail), smoothed over a small neighbourhood so the
   choice doesn't flicker pixel to pixel.
3. For each pixel, take the value from the image that was sharpest there.

Also returns a depth map: which image each pixel came from (0 = first).
Brighter in the depth map = later in the stack.

Run directly for a self-test:
    python -m camcontrol.processing.focus_stack
"""

import cv2
import numpy as np

from camcontrol.processing.common import align_all, as_bgr, as_gray, check_same_size


def sharpness(image: np.ndarray, smooth_sigma: float) -> np.ndarray:
    gray = cv2.GaussianBlur(as_gray(image), (3, 3), 0)  # ignore single-pixel noise
    lap = np.abs(cv2.Laplacian(gray, cv2.CV_32F, ksize=3))
    return cv2.GaussianBlur(lap, (0, 0), smooth_sigma)


def focus_stack(images: list[np.ndarray], align: bool = True) -> tuple[np.ndarray, np.ndarray]:
    """Combine a focus series. Returns (result BGR image, depth map 8-bit)."""
    if len(images) < 2:
        raise ValueError("Focus stacking needs at least 2 images.")
    check_same_size(images)
    images = [as_bgr(im) for im in images]
    if align:
        images = align_all(images)

    w = images[0].shape[1]
    sigma = max(2.0, w / 500)  # ~4 px on a 1080p frame
    scores = np.stack([sharpness(im, sigma) for im in images])  # (n, h, w)
    best = np.argmax(scores, axis=0)                            # (h, w)

    stack = np.stack(images)                                    # (n, h, w, 3)
    result = np.take_along_axis(stack, best[None, :, :, None], axis=0)[0]

    depth = (best * (255 / (len(images) - 1))).astype(np.uint8)
    return result, depth


if __name__ == "__main__":
    # A pattern where the left half is sharp in image 0, the right half in image 1.
    rng = np.random.default_rng(0)
    sharp = (rng.random((200, 300)) * 255).astype(np.uint8)
    blurry = cv2.GaussianBlur(sharp, (0, 0), 4)
    a = sharp.copy(); a[:, 150:] = blurry[:, 150:]
    b = sharp.copy(); b[:, :150] = blurry[:, :150]

    result, depth = focus_stack([a, b], align=False)
    err = np.abs(as_gray(result).astype(int) - sharp.astype(int))[:, 10:-10].mean()
    print(f"mean difference from the all-sharp image: {err:.1f} (0-255)")
    assert err < 10
    assert depth[:, :100].mean() < 50 and depth[:, 200:].mean() > 200
    print("focus_stack OK")
