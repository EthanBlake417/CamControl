"""Open the microscope camera, grab frames, and set exposure / gain / etc.

Findings from Phase 0 (tools/probe_camera.py, ffmpeg -list_options, and
camcontrol/uvc_controls.py):
  - Windows name: "MC802 USB2.0 Camera", index 0 on this PC, using the
    standard Microsoft UVC driver (usbvideo.sys). No vendor driver.
  - Largest USB mode is 1920x1080 (MJPG 30 fps, or YUY2 7 fps). There is no
    3264x1836 mode over USB; HD2's 3264x1836 TIFs are 1080p scaled up.
    Real 6MP stills only come from the camera's SD card.
  - Frames come through OpenCV with DirectShow. DirectShow only picks the
    fast MJPG format if the frame size is set BEFORE the FOURCC (the other
    order silently falls back to 5.7 fps YUY2).
  - Controls are set through camcontrol.uvc_controls (the Windows camera
    control interfaces), not OpenCV, because OpenCV caps gain at 32 and
    can't recover exposure after HD2 has used the camera (see below).
  - All control values are stored in the camera, so they persist after this
    program closes, and other apps (HD2, OBS) will see them too.

Controls that work (ranges as reported by the camera):
  exposure     -13 .. -3   (see below)
  gain           0 .. 63   (default 48)
  contrast       0 .. 15
  saturation     0 .. 15
  sharpness      0 .. 7
Brightness and gamma are listed by the camera but ignore writes.

Exposure:
  Values are log2(seconds): -8 means 1/256 s. This camera actually exposes
  about 10x longer than that (measured from frame timing: -4 gives 625 ms
  frames, not 62.5 ms), so real exposure ~= 10 * 2**value seconds, i.e.
  about 1.2 ms to 1.25 s.
  After HD2 has used the camera, exposure writes with the "manual" flag are
  ignored until one write with no flags goes through. set_exposure() does
  that every time. The camera always reports exposure as "auto", even when
  manual values are clearly in effect, so that flag can't be trusted.

Run this file directly for a quick check that the camera opens:
    python -m camcontrol.camera
"""

import os
import time

# Must be set before importing cv2.
os.environ.setdefault("OPENCV_LOG_LEVEL", "ERROR")

import cv2  # noqa: E402
import numpy as np  # noqa: E402

from camcontrol.uvc_controls import CameraControls  # noqa: E402

DEFAULT_INDEX = 0
DEFAULT_WIDTH = 1920
DEFAULT_HEIGHT = 1080

# Controls the app offers. Order here is the order shown in the GUI.
ADJUSTABLE = ["exposure", "gain", "contrast", "saturation", "sharpness"]


def exposure_seconds(value: float) -> float:
    """Approximate real exposure time for an exposure value (see above)."""
    return 10 * 2.0 ** value


def format_exposure(value: float) -> str:
    """Human-readable exposure, e.g. '39 ms' or '1.25 s'."""
    s = exposure_seconds(value)
    return f"{s:.2f} s" if s >= 1 else f"{s * 1000:.0f} ms"


class Camera:
    """The microscope camera: frames from OpenCV, controls from uvc_controls.

    Usage:
        cam = Camera()
        frame = cam.read()
        cam.set_exposure(-6)
        cam.set("contrast", 10)
        cam.close()
    """

    def __init__(
        self,
        index: int = DEFAULT_INDEX,
        width: int = DEFAULT_WIDTH,
        height: int = DEFAULT_HEIGHT,
    ):
        self.cap = cv2.VideoCapture(index, cv2.CAP_DSHOW)
        if not self.cap.isOpened():
            raise RuntimeError(
                f"Could not open camera {index}. Close HD2 / OBS and try again."
            )

        # Order matters: size first, then MJPG (see module docstring).
        self.cap.set(cv2.CAP_PROP_FRAME_WIDTH, width)
        self.cap.set(cv2.CAP_PROP_FRAME_HEIGHT, height)
        self.cap.set(cv2.CAP_PROP_FOURCC, cv2.VideoWriter_fourcc(*"MJPG"))

        frame = self.read()
        if frame is None:
            self.close()
            raise RuntimeError(f"Camera {index} opened but returned no frame.")

        actual_h, actual_w = frame.shape[:2]
        if (actual_w, actual_h) != (width, height):
            print(f"Note: asked for {width}x{height}, camera gave {actual_w}x{actual_h}")

        # A second handle on the same device, for its controls.
        self.controls = CameraControls(index)
        self.ranges = {name: self.controls.range(name) for name in ADJUSTABLE}

        # Re-apply the current exposure so it's in effect even if HD2 left
        # the camera ignoring manual exposure.
        self.set_exposure(self.exposure)

    def read(self):
        """Return the next frame, or None if no valid frame arrived.

        With exposures of ~1 s or longer, OpenCV's DirectShow reader times
        out and hands back an all-black frame. Those are skipped here.
        """
        ok, frame = self.cap.read()
        if not ok or not np.any(frame):
            return None
        return frame

    # --- controls ------------------------------------------------------------

    def get(self, name: str) -> int:
        return self.controls.get(name)[0]

    def set(self, name: str, value: float) -> int:
        """Set a control (clamped to its range). Returns the value read back."""
        r = self.ranges.get(name)
        if r is not None:
            value = max(r["min"], min(r["max"], round(value)))
        if name == "exposure":
            # Workaround (see module docstring): a no-flags write first.
            self.controls.set(name, value, flags=0)
        self.controls.set(name, value)
        return self.get(name)

    def values(self) -> dict[str, int]:
        """Current value of every adjustable control."""
        return {name: self.get(name) for name in ADJUSTABLE}

    # Shortcuts for the two used most.
    @property
    def exposure(self) -> int:
        return self.get("exposure")

    def set_exposure(self, value: float) -> int:
        return self.set("exposure", value)

    @property
    def gain(self) -> int:
        return self.get("gain")

    def set_gain(self, value: float) -> int:
        return self.set("gain", value)

    def close(self):
        self.cap.release()


if __name__ == "__main__":
    cam = Camera()
    t0 = time.perf_counter()
    n = 60
    for _ in range(n):
        frame = cam.read()
    elapsed = time.perf_counter() - t0
    h, w = frame.shape[:2]
    print(f"Camera OK: {w}x{h}, {n / elapsed:.1f} fps")
    for name, value in cam.values().items():
        r = cam.ranges[name]
        extra = f"  (~{format_exposure(value)})" if name == "exposure" else ""
        print(f"  {name:11} {value:4}  range {r['min']}..{r['max']}{extra}")
    cam.close()
