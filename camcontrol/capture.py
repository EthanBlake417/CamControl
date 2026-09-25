"""Still capture: average several frames, optionally resize, save with metadata.

How HD2 does it (from its config files and DLLs, 2026-09-24):
  - HD2 talks to this camera through the standard UVC interface, same as us.
  - It has separate "preview" and "save" resolutions, and an interpolation
    setting. Its 3264x1836 files are 1080p frames scaled up on save.
  - Its long exposures and low-noise captures come from software frame
    averaging / integration (settings PROCESS_AVERAGE, PROCESS_INTEGRAL).

So capture here works the same way:
  - Average N frames (reduces noise; N=1 is a plain snapshot).
  - Save at the native 1920x1080, or scaled up to 3264x1836 to match HD2
    files. Scaling up adds pixels, not detail, and changes the µm/px scale,
    so the save size is recorded in the metadata.

Each capture writes an image plus a .json sidecar with the settings used.

Run directly to take one averaged capture without the viewer:
    python -m camcontrol.capture
"""

import json
from datetime import datetime
from pathlib import Path

import cv2
import numpy as np

from camcontrol.camera import Camera, exposure_seconds

CAPTURE_DIR = Path(__file__).resolve().parent.parent / "captures"

NATIVE_SIZE = (1920, 1080)
HD2_SIZE = (3264, 1836)


def grab_average(cam: Camera, n_frames: int, max_attempts: int | None = None):
    """Grab n_frames valid frames and return their average as uint8.

    Frames are summed as float32 so nothing clips before dividing.
    Black/invalid frames (see Camera.read) are skipped.
    """
    if max_attempts is None:
        max_attempts = n_frames * 3 + 5
    total = None
    got = 0
    for _ in range(max_attempts):
        frame = cam.read()
        if frame is None:
            continue
        if total is None:
            total = np.zeros(frame.shape, np.float32)
        total += frame
        got += 1
        if got == n_frames:
            break
    if got == 0:
        raise RuntimeError("No valid frames from camera.")
    return np.clip(total / got + 0.5, 0, 255).astype(np.uint8), got


def save_capture(
    image,
    *,
    exposure: float,
    gain: float,
    frames_averaged: int,
    save_size: tuple[int, int] = NATIVE_SIZE,
    fmt: str = "tif",
    folder: Path = CAPTURE_DIR,
    prefix: str = "cap",
) -> Path:
    """Save an image (resized to save_size if needed) plus a JSON sidecar.

    Returns the image path.
    """
    folder.mkdir(parents=True, exist_ok=True)
    now = datetime.now()
    stem = f"{prefix}_{now:%Y%m%d_%H%M%S}"
    path = folder / f"{stem}.{fmt}"
    # Don't overwrite if two captures land in the same second.
    n = 1
    while path.exists():
        path = folder / f"{stem}_{n}.{fmt}"
        n += 1

    src_h, src_w = image.shape[:2]
    if (src_w, src_h) != save_size:
        # Bicubic is a good general-purpose choice for scaling up.
        image = cv2.resize(image, save_size, interpolation=cv2.INTER_CUBIC)

    if not cv2.imwrite(str(path), image):
        raise RuntimeError(f"Could not write {path}")

    meta = {
        "timestamp": now.isoformat(timespec="seconds"),
        "source_size": [src_w, src_h],
        "saved_size": list(save_size),
        "upscaled": save_size != (src_w, src_h),
        "exposure_value": exposure,
        "exposure_seconds_approx": exposure_seconds(exposure),
        "gain": gain,
        "frames_averaged": frames_averaged,
        # Filled in once calibration (Phase 2) exists.
        "calibration": None,
    }
    path.with_suffix(".json").write_text(json.dumps(meta, indent=2))
    return path


if __name__ == "__main__":
    cam = Camera()
    img, got = grab_average(cam, 8)
    out = save_capture(img, exposure=cam.exposure, gain=cam.gain, frames_averaged=got)
    cam.close()
    print(f"Saved {out} (average of {got} frames)")
