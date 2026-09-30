"""Stitching: join overlapping images into one large field of view.

Move the sample between shots so neighbouring images overlap by about a
third, then stitch. Uses OpenCV's Stitcher:

- "scans" mode (default) assumes a flat sample moved under the camera,
  which is the microscope case. Images may shift and rotate, no perspective.
- "panorama" mode is for a camera turning to look around a scene.

Run directly for a self-test:
    python -m camcontrol.processing.stitch
"""

import cv2
import numpy as np

from camcontrol.processing.common import as_bgr

MODES = {"scans": cv2.Stitcher_SCANS, "panorama": cv2.Stitcher_PANORAMA}

ERRORS = {
    cv2.Stitcher_ERR_NEED_MORE_IMGS:
        "Couldn't find enough overlap between the images. Neighbouring images "
        "should overlap by about a third and show some detail (not blank areas).",
    cv2.Stitcher_ERR_HOMOGRAPHY_EST_FAIL:
        "Couldn't work out how the images fit together. Try more overlap, or the other mode.",
    cv2.Stitcher_ERR_CAMERA_PARAMS_ADJUST_FAIL:
        "Couldn't refine how the images fit together. Try the other mode.",
}


def crop_to_content(image: np.ndarray) -> np.ndarray:
    """Cut off the black border around a stitched image."""
    gray = image if image.ndim == 2 else image.max(axis=2)
    ys, xs = np.nonzero(gray)
    if len(xs) == 0:
        return image
    return image[ys.min():ys.max() + 1, xs.min():xs.max() + 1]


def stitch(images: list[np.ndarray], mode: str = "scans", crop: bool = True) -> np.ndarray:
    if len(images) < 2:
        raise ValueError("Stitching needs at least 2 images.")
    stitcher = cv2.Stitcher_create(MODES[mode])
    # The stitcher crashes on non-contiguous arrays (e.g. crops made by slicing).
    images = [np.ascontiguousarray(as_bgr(im)) for im in images]
    try:
        status, result = stitcher.stitch(images)
    except cv2.error as e:
        raise RuntimeError(f"Stitching failed inside OpenCV: {e}") from e
    if status != cv2.Stitcher_OK:
        raise RuntimeError(ERRORS.get(status, f"Stitching failed (OpenCV status {status})."))
    return crop_to_content(result) if crop else result


if __name__ == "__main__":
    # Cut three overlapping tiles out of one detailed picture and stitch them back.
    rng = np.random.default_rng(1)
    big = cv2.GaussianBlur((rng.random((300, 700)) * 255).astype(np.uint8), (0, 0), 1.5)
    big = cv2.cvtColor(cv2.normalize(big, None, 0, 255, cv2.NORM_MINMAX), cv2.COLOR_GRAY2BGR)
    for i in range(40):  # some shapes, for features to match
        cv2.circle(big, tuple(int(v) for v in rng.integers(0, (700, 300))), int(rng.integers(5, 25)),
                   tuple(int(v) for v in rng.integers(0, 255, 3)), -1)
    tiles = [big[:, 0:300], big[:, 200:500], big[:, 400:700]]
    result = stitch(tiles)
    print(f"tiles 3 x 300x300 -> stitched {result.shape[1]}x{result.shape[0]} (original 700x300)")
    assert abs(result.shape[1] - 700) < 40 and abs(result.shape[0] - 300) < 40
    print("stitch OK")
