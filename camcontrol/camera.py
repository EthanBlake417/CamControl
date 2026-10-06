"""Open the microscope camera, grab frames, and set exposure / gain / etc.

Works with any standard USB (UVC) camera Windows lists. Camera(index) opens
the camera at its largest frame size (from uvc_controls.list_modes), or the
size asked for. Cameras differ in which controls they have: ranges[name] is
None for any the camera doesn't support, and values() leaves those out.
The notes below are about the camera this was written for (the MC802).

Findings from Phase 0 (tools/probe_camera.py, ffmpeg -list_options, and
camcontrol/uvc_controls.py):
  - Windows name: "MC802 USB2.0 Camera", index 0 on this PC, using the
    standard Microsoft UVC driver (usbvideo.sys). No vendor driver.
  - Largest USB mode is 1920x1080 (MJPG 30 fps, or YUY2 7 fps). There is no
    3264x1836 mode over USB; HD2's 3264x1836 TIFs are 1080p scaled up.
    The camera's "6MP" SD-card stills are 1080p enlarged in the camera too,
    so 1920x1080 is the real resolution.
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

Long exposures (beyond the camera's 1.25 s):
  The exposure range offered goes EXTRA_EXPOSURE_STEPS past the camera's
  maximum: -2 ~ 2.5 s, -1 ~ 5 s, 0 ~ 10 s, 1 ~ 20 s. Above -3 the camera
  stays at its longest exposure and frames are added together in software,
  2, 4, 8 or 16 of them, which brightens like a real longer exposure. HD2's
  "10 s" exposures work the same way.
  - read() (live view, video) returns a running sum of the last N frames,
    so the picture still updates every 1.25 s.
  - grab_exposure() (captures) sums N fresh frames: one true long exposure.

Run this file directly for a quick check that the camera opens:
    python -m camcontrol.camera
"""

import os
import time
from collections import deque

# Must be set before importing cv2.
os.environ.setdefault("OPENCV_LOG_LEVEL", "ERROR")

import cv2  # noqa: E402
import numpy as np  # noqa: E402

from camcontrol.uvc_controls import CameraControls, list_modes  # noqa: E402

DEFAULT_INDEX = 0
# Asked for when the camera won't list its sizes.
DEFAULT_WIDTH = 1920
DEFAULT_HEIGHT = 1080

# Controls the app offers. Order here is the order shown in the GUI.
ADJUSTABLE = ["exposure", "gain", "contrast", "saturation", "sharpness"]
# Exposure steps offered past the camera's maximum, done by adding frames
# (see "Long exposures" above). 4 steps: up to 16 frames, ~20 s.
EXTRA_EXPOSURE_STEPS = 4
# The camera's real exposure range (Phase 0), used if it reports a broken one.
EXPOSURE_RANGE = (-13, -3)
EXPOSURE_DEFAULT = -6
# How long to wait for a first frame when opening. Several 1.25 s exposures,
# some of which time out (see read_raw).
OPEN_TIMEOUT_S = 6.0


def exposure_seconds(value: float) -> float:
    """Approximate real exposure time for an exposure value (see above)."""
    return 10 * 2.0 ** value


def format_exposure(value: float) -> str:
    """Human-readable exposure, e.g. '39 ms' or '1.25 s'."""
    s = exposure_seconds(value)
    return f"{s:.2f} s" if s >= 1 else f"{s * 1000:.0f} ms"


def frame_sizes(modes: list[dict]) -> list[tuple[int, int, float | None]]:
    """The distinct frame sizes in modes (from uvc_controls.list_modes), largest
    first, as (width, height, fastest fps at that size in any format)."""
    best: dict[tuple[int, int], float | None] = {}
    for m in modes:
        key = (m["width"], m["height"])
        fps = m["fps"]
        if key not in best or (fps or 0) > (best[key] or 0):
            best[key] = fps
    return sorted(((w, h, fps) for (w, h), fps in best.items()), key=lambda s: (-s[0] * s[1], -s[0]))


class Camera:
    """The microscope camera: frames from OpenCV, controls from uvc_controls.

    Usage:
        cam = Camera()
        frame = cam.read()
        cam.set_exposure(-6)
        cam.set("contrast", 10)
        cam.close()
    """

    def __init__(self, index: int = DEFAULT_INDEX, size: tuple[int, int] | None = None):
        """size: (width, height) to ask for; None = the largest the camera offers."""
        # Sizes and formats the camera offers (empty if it won't say).
        self.modes = list_modes(index)
        sizes = frame_sizes(self.modes)
        if size is None:
            size = sizes[0][:2] if sizes else (DEFAULT_WIDTH, DEFAULT_HEIGHT)
        width, height = size

        self.cap = cv2.VideoCapture(index, cv2.CAP_DSHOW)
        if not self.cap.isOpened():
            raise RuntimeError(
                f"Could not open camera {index}. Close HD2 / OBS and try again."
            )

        # Order matters: size first, then MJPG (see module docstring). MJPG is
        # usually the fast format, but only ask for it if the camera has it at
        # this size (or won't say).
        self.cap.set(cv2.CAP_PROP_FRAME_WIDTH, width)
        self.cap.set(cv2.CAP_PROP_FRAME_HEIGHT, height)
        if not self.modes or any(m["fourcc"] == "MJPG" and (m["width"], m["height"]) == (width, height)
                                 for m in self.modes):
            self.cap.set(cv2.CAP_PROP_FOURCC, cv2.VideoWriter_fourcc(*"MJPG"))

        # At long exposures (the camera remembers its last setting) the first
        # reads time out and come back black, so keep trying for a while.
        frame = None
        deadline = time.perf_counter() + OPEN_TIMEOUT_S
        while frame is None and time.perf_counter() < deadline:
            frame = self.read_raw()
        if frame is None:
            self.close()
            raise RuntimeError(f"Camera {index} opened but returned no frame.")

        actual_h, actual_w = frame.shape[:2]
        if (actual_w, actual_h) != (width, height):
            print(f"Note: asked for {width}x{height}, camera gave {actual_w}x{actual_h}")
        self.size = (actual_w, actual_h)

        # A second handle on the same device, for its controls.
        self.controls = CameraControls(index)
        self.name = self.controls.name
        self.ranges = {name: self._probe(name) for name in ADJUSTABLE}
        # The camera sometimes reports a nonsense exposure range (e.g. 3..3,
        # not even containing its current value); use the measured one then.
        exp = self.ranges.get("exposure")
        if exp and not (exp["min"] < exp["max"] and exp["min"] <= self.controls.get("exposure")[0] <= exp["max"]):
            self.ranges["exposure"] = exp = {**exp, "min": EXPOSURE_RANGE[0], "max": EXPOSURE_RANGE[1],
                                             "default": EXPOSURE_DEFAULT}
        # Offer longer exposures than the camera can do, by adding frames.
        self.hw_exposure_max = exp["max"] if exp else None
        if exp:
            self.ranges["exposure"] = {**exp, "max": exp["max"] + EXTRA_EXPOSURE_STEPS}
        self.frames_summed = 1  # 1 = a normal exposure; 2, 4, 8, 16 = long exposure
        self._recent: deque[np.ndarray] = deque()  # last frames_summed frames, for read()
        self._sum: np.ndarray | None = None         # their sum (int32)

        # Re-apply the current exposure so it's in effect even if HD2 left
        # the camera ignoring manual exposure.
        if exp:
            self.set_exposure(self.exposure)

    def _probe(self, name: str) -> dict | None:
        """A control's range, or None if the camera doesn't have it (or
        reports a range but can't be read)."""
        try:
            r = self.controls.range(name)
            if r is not None:
                self.controls.get(name)
            return r
        except Exception:
            return None

    def read_raw(self):
        """Return the next frame from the camera, or None if no valid frame arrived.

        With exposures of ~1 s or longer, OpenCV's DirectShow reader times
        out and hands back an all-black frame. Those are skipped here.
        """
        ok, frame = self.cap.read()
        if not ok or not np.any(frame):
            return None
        return frame

    def read(self):
        """The next live frame, or None. During a long exposure: the sum of the
        last frames_summed frames, updated with every new frame."""
        frame = self.read_raw()
        if frame is None or self.frames_summed == 1:
            return frame
        if self._sum is None or self._sum.shape != frame.shape:
            self._reset_sum()
            self._sum = np.zeros(frame.shape, np.int32)
        self._recent.append(frame)
        self._sum += frame
        if len(self._recent) > self.frames_summed:
            self._sum -= self._recent.popleft()
        # Until enough frames have arrived, scale up so brightness is right.
        scale = self.frames_summed / len(self._recent)
        return np.clip(self._sum * scale, 0, 255).astype(np.uint8)

    def grab_exposure(self, max_attempts: int | None = None):
        """One complete exposure from fresh frames (for captures), or None.
        Long exposures add frames_summed frames; bright parts clip at 255,
        as they would in a real long exposure."""
        n = self.frames_summed
        if n == 1:
            return self.read_raw()
        total = None
        got = 0
        for _ in range(max_attempts or n * 2 + 3):
            frame = self.read_raw()
            if frame is None:
                continue
            total = frame.astype(np.int32) if total is None else total + frame
            got += 1
            if got == n:
                break
        if total is None:
            return None
        total = total * (n / got)  # if a frame was missed, scale to the full exposure
        return np.clip(total, 0, 255).astype(np.uint8)

    def _reset_sum(self):
        self._recent.clear()
        self._sum = None

    # --- controls ------------------------------------------------------------

    def get(self, name: str) -> int:
        if self.ranges.get(name) is None:
            raise RuntimeError(f"This camera has no {name} control")
        value = self.controls.get(name)[0]
        if name == "exposure" and self.frames_summed > 1:
            value += self.frames_summed.bit_length() - 1  # + log2(frames summed)
        return value

    def set(self, name: str, value: float) -> int:
        """Set a control (clamped to its range). Returns the value read back."""
        r = self.ranges.get(name)
        if r is None:
            raise RuntimeError(f"This camera has no {name} control")
        value = max(r["min"], min(r["max"], round(value)))
        if name == "exposure":
            # Past the camera's maximum: longest real exposure, frames added.
            extra = max(0, value - self.hw_exposure_max) if self.hw_exposure_max is not None else 0
            if 2 ** extra != self.frames_summed:
                self.frames_summed = 2 ** extra
                self._reset_sum()
            value -= extra
            # Workaround (see module docstring): a no-flags write first.
            self.controls.set(name, value, flags=0)
        self.controls.set(name, value)
        return self.get(name)

    def values(self) -> dict[str, int]:
        """Current value of every adjustable control the camera has."""
        return {name: self.get(name) for name in ADJUSTABLE if self.ranges.get(name) is not None}

    # Shortcuts for the two used most. None if the camera doesn't have them.
    @property
    def exposure(self) -> int | None:
        return self.get("exposure") if self.ranges.get("exposure") else None

    def set_exposure(self, value: float) -> int:
        return self.set("exposure", value)

    @property
    def gain(self) -> int | None:
        return self.get("gain") if self.ranges.get("gain") else None

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
    print(f"Camera OK: {cam.name}, {w}x{h}, {n / elapsed:.1f} fps")
    for name, value in cam.values().items():
        r = cam.ranges[name]
        extra = f"  (~{format_exposure(value)})" if name == "exposure" else ""
        print(f"  {name:11} {value:4}  range {r['min']}..{r['max']}{extra}")
    cam.close()
