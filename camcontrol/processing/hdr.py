"""HDR by exposure fusion: combine images taken at different exposures.

A single exposure can't show both a bright reflective area and dark
shadows. Take the same view at a few exposures (e.g. short, medium, long)
and exposure fusion (Mertens et al.) blends them, favouring each image
where it's well exposed, colorful and detailed. The result is a normal
8-bit image; no exposure times or tone mapping are needed.

Run directly for a self-test:
    python -m camcontrol.processing.hdr
"""

import cv2
import numpy as np

from camcontrol.processing.common import as_bgr, check_same_size


def exposure_fusion(images: list[np.ndarray], align: bool = True) -> np.ndarray:
    if len(images) < 2:
        raise ValueError("HDR needs at least 2 images at different exposures.")
    check_same_size(images)
    images = [as_bgr(im) for im in images]
    if align:
        # MTB alignment compares brightness patterns, so it copes with images
        # that are much darker or brighter than each other. Shifts only.
        cv2.createAlignMTB().process(images, images)
    fused = cv2.createMergeMertens().process(images)  # float, about 0..1
    return np.clip(fused * 255 + 0.5, 0, 255).astype(np.uint8)


if __name__ == "__main__":
    # A scene with a range too wide for one exposure: gradient from dark to very bright.
    scene = np.tile(np.linspace(0, 4, 300, dtype=np.float32), (200, 1))
    exposures = [np.clip(scene * k * 255, 0, 255).astype(np.uint8) for k in (0.25, 1, 4)]
    fused = as_bgr(exposure_fusion(exposures, align=False))[..., 0].astype(int)
    # Should increase left to right with no big flat (clipped) areas.
    steps = np.diff(fused[100, 5:-5])
    flat_fraction = np.mean(steps == 0)
    print(f"fused: {fused[100, 0]} .. {fused[100, -1]}, flat steps {flat_fraction:.0%}")
    assert fused[100, -1] > fused[100, 0] + 100
    print("hdr OK")
