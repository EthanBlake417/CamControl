"""Flat-field correction: even out uneven lighting and remove fixed dust spots.

Take a "flat" reference: an image of an empty, evenly lit field (a blank
slide, or white paper slightly out of focus) with the same lighting and
zoom as your samples. Anything that isn't even in the flat (vignetting,
a bright spot from the light, dust on the sensor) is a property of the
setup, not the sample. Correction divides it out:

    corrected = image * mean(flat) / flat        (per color channel)

so a pixel that the flat shows as 20% darker than average is brightened by
25%. This also balances the color if the flat has a tint.

Run directly for a self-test:
    python -m camcontrol.processing.flatfield
"""

import cv2
import numpy as np

from camcontrol.processing.common import as_bgr


class FlatField:
    def __init__(self, flat: np.ndarray, name: str = "flat"):
        self.name = name
        flat = as_bgr(flat).astype(np.float32)
        # A light blur takes out pixel noise, so the correction doesn't add it
        # to every image. Dust shadows are much wider than this and are kept.
        flat = cv2.GaussianBlur(flat, (0, 0), 2)
        flat = np.maximum(flat, 1.0)  # no divide-by-zero in black corners
        means = flat.reshape(-1, 3).mean(axis=0)
        self._gain = (means / flat).astype(np.float32)
        self._resized: dict[tuple[int, int], np.ndarray] = {}

    @property
    def size(self) -> tuple[int, int]:
        h, w = self._gain.shape[:2]
        return w, h

    def _gain_for(self, h: int, w: int) -> np.ndarray:
        """The gain map at the image's size.

        A flat taken on 1920x1080 live frames also works on 3264x1836 files,
        since those are the same frames scaled up.
        """
        if (w, h) == self.size:
            return self._gain
        if (w, h) not in self._resized:
            self._resized[(w, h)] = cv2.resize(self._gain, (w, h), interpolation=cv2.INTER_LINEAR)
        return self._resized[(w, h)]

    def apply(self, image: np.ndarray) -> np.ndarray:
        """Corrected copy of an 8-bit image (grayscale input comes back BGR)."""
        image = as_bgr(image)
        gain = self._gain_for(*image.shape[:2])
        # uint8 x float32 -> uint8 in one step (rounds and clips); fast enough
        # for every live frame (~4 ms at 1920x1080).
        return cv2.multiply(image, gain, dtype=cv2.CV_8U)


if __name__ == "__main__":
    # A flat, even sample, seen through a lens that darkens the corners.
    h, w = 200, 300
    yy, xx = np.mgrid[0:h, 0:w]
    vignette = 1 - 0.5 * (((xx - w / 2) / w) ** 2 + ((yy - h / 2) / h) ** 2) * 2
    flat = np.dstack([200 * vignette] * 3).astype(np.uint8)
    sample = np.dstack([120 * vignette] * 3).astype(np.uint8)

    ff = FlatField(flat)
    fixed = ff.apply(sample)
    before = sample[..., 0].std()
    after = fixed[..., 0][10:-10, 10:-10].std()
    print(f"unevenness (std) before {before:.1f}, after {after:.1f}")
    assert after < before / 5, "correction should flatten the image"
    # Works on a scaled-up copy too.
    big = cv2.resize(sample, (600, 400))
    assert ff.apply(big).shape == (400, 600, 3)
    print("flatfield OK")
