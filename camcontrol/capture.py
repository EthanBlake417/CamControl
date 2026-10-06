"""Still capture: average several frames, optionally resize, save with metadata.

How HD2 does it (from its config files and DLLs, 2026-09-24):
  - HD2 talks to this camera through the standard UVC interface, same as us.
  - It has separate "preview" and "save" resolutions, and an interpolation
    setting. Its 3264x1836 files are 1080p frames scaled up on save.
  - Its long exposures and low-noise captures come from software frame
    averaging / integration (settings PROCESS_AVERAGE, PROCESS_INTEGRAL).

So capture here works the same way:
  - Average N frames (reduces noise; N=1 is a plain snapshot).
  - Save at the camera's own size, or scaled up 1.7x to match HD2 files
    (1920x1080 becomes 3264x1836). Scaling up adds pixels, not detail, and
    changes the µm/px scale, so the save size is recorded in the metadata.

Each capture writes an image plus a .json sidecar with the settings used.
Files are named name-001.tif, name-002.tif, ... when a name is given, or
by date and time when it isn't (see next_capture_path).

Run directly to take one averaged capture without the viewer:
    python -m camcontrol.capture
"""

import json
import re
from datetime import datetime
from pathlib import Path

import cv2
import numpy as np

from camcontrol.camera import Camera, exposure_seconds
from camcontrol.image_io import save_image_file

CAPTURE_DIR = Path(__file__).resolve().parent.parent / "captures"

# HD2 saves the MC802's 1920x1080 frames at 3264x1836: 1.7x larger.
HD2_SCALE = 3264 / 1920


def scaled_size(width: int, height: int, scale: float) -> tuple[int, int]:
    """Frame size after scaling (1.0 = unchanged)."""
    return round(width * scale), round(height * scale)


def grab_average(cam: Camera, n_frames: int, max_attempts: int | None = None):
    """Grab n_frames valid frames and return their average as uint8.

    Frames are summed as float32 so nothing clips before dividing.
    Black/invalid frames (see Camera.read_raw) are skipped. With a long
    exposure, each "frame" is itself several camera frames added together
    (Camera.grab_exposure).
    """
    if max_attempts is None:
        max_attempts = n_frames * 3 + 5
    total = None
    got = 0
    for _ in range(max_attempts):
        frame = cam.grab_exposure()
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


def clean_name(name: str) -> str:
    """Make a user-typed capture name safe as a Windows file name."""
    name = re.sub(r'[<>:"/\\|?*\x00-\x1f]', "_", name.strip())
    return name.rstrip(". ")  # Windows doesn't allow trailing dots/spaces


def next_capture_path(folder: Path, name: str = "", fmt: str = "tif", prefix: str = "cap") -> Path:
    """Where the next capture will be saved.

    With a name:    name-001.tif, name-002.tif, ... continuing after the highest
                    number already in the folder (any extension), so nothing is
                    overwritten. Same style as HD2 ("..._bright-001.tif").
    Without a name: cap_YYYYMMDD_HHMMSS.tif (with _1, _2 if taken).
    """
    folder = Path(folder)
    name = clean_name(name)
    if name:
        pattern = re.compile(rf"^{re.escape(name)}-(\d+)\.[^.]+$", re.IGNORECASE)
        numbers = []
        if folder.is_dir():
            for p in folder.iterdir():
                m = pattern.match(p.name)
                if m:
                    numbers.append(int(m.group(1)))
        return folder / f"{name}-{max(numbers, default=0) + 1:03d}.{fmt}"

    stem = f"{prefix}_{datetime.now():%Y%m%d_%H%M%S}"
    path = folder / f"{stem}.{fmt}"
    n = 1
    while path.exists():  # two captures in the same second
        path = folder / f"{stem}_{n}.{fmt}"
        n += 1
    return path


def save_capture(
    image,
    *,
    exposure: float,
    gain: float,
    frames_averaged: int,
    scale: float = 1.0,
    fmt: str = "tif",
    folder: Path = CAPTURE_DIR,
    name: str = "",
    prefix: str = "cap",
    extra: dict | None = None,
) -> Path:
    """Save an image (scaled up by scale, if not 1) plus a JSON sidecar.

    extra: more entries for the sidecar (e.g. the flat-field used).

    See next_capture_path() for how the file is named. Returns the image path.
    """
    folder = Path(folder)
    folder.mkdir(parents=True, exist_ok=True)
    now = datetime.now()
    path = next_capture_path(folder, name, fmt, prefix)

    src_h, src_w = image.shape[:2]
    save_size = scaled_size(src_w, src_h, scale)
    if (src_w, src_h) != save_size:
        # Bicubic is a good general-purpose choice for scaling up.
        image = cv2.resize(image, save_size, interpolation=cv2.INTER_CUBIC)

    save_image_file(path, image)

    meta = {
        "timestamp": now.isoformat(timespec="seconds"),
        "source_size": [src_w, src_h],
        "saved_size": list(save_size),
        "upscaled": save_size != (src_w, src_h),
        "exposure_value": exposure,
        "exposure_seconds_approx": exposure_seconds(exposure) if exposure is not None else None,
        "gain": gain,
        "frames_averaged": frames_averaged,
        # Filled in once calibration (Phase 2) exists.
        "calibration": None,
        **(extra or {}),
    }
    path.with_suffix(".json").write_text(json.dumps(meta, indent=2))
    return path


if __name__ == "__main__":
    cam = Camera()
    img, got = grab_average(cam, 8)
    out = save_capture(img, exposure=cam.exposure, gain=cam.gain, frames_averaged=got)
    cam.close()
    print(f"Saved {out} (average of {got} frames)")
