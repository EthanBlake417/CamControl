"""A fake camera worker for tests: same signals and calls as
camcontrol.gui.camera_worker.CameraWorker, but frames come from synthetic
fiber images (about 30 fps, honouring frame acknowledgement like the real
one), controls are just remembered, and captures are written with the real
save_capture."""

import time

from PySide6.QtCore import QThread, Signal

import synth
from camcontrol.capture import save_capture

DEVICES = [("FakeCam A", 0), ("FakeCam B", 0)]
WORKERS: list["FakeWorker"] = []  # every one made (tests check them)
_FRAMES = {}


def frame_for(index: int):
    """The frame camera number index sends: a synthetic Panda fiber, and its truth."""
    if index not in _FRAMES:
        _FRAMES[index] = synth.make(10 + index, 0.5, "relief")
    return _FRAMES[index]


class FakeWorker(QThread):
    opened = Signal(dict)
    open_failed = Signal(str)
    frame_ready = Signal(object)
    settings_changed = Signal(dict)
    capture_started = Signal(int, float)
    capture_done = Signal(str)
    error = Signal(str)
    recording_changed = Signal(bool, str)
    recording_progress = Signal(float, int)
    timelapse_progress = Signal(int, int, float, str)
    timelapse_finished = Signal(str)

    def __init__(self, index=0, size=None, parent=None):
        super().__init__(parent)
        self.index, self.size = index, size
        self._running = True
        self._awaiting = False
        self._queue: list = []
        self.controls: dict = {}
        self.captures: list[str] = []
        self.flat = "never set"
        self.frames_sent = 0
        WORKERS.append(self)

    def run(self):
        if self.index is None:
            self.open_failed.emit("Camera not found.")
            return
        frame = frame_for(self.index)[0]
        h, w = frame.shape[:2]
        self.opened.emit({"name": DEVICES[self.index][0], "size": (w, h),
                          "modes": [{"width": w, "height": h, "fps": 30.0, "format": "MJPG"}],
                          "ranges": {"exposure": {"min": -13, "max": 0}, "gain": {"min": 0, "max": 100},
                                     "exposure_us": {"min": 1221, "max": 10000000, "default": 156250}}})
        self.settings_changed.emit({"exposure": -6, "gain": 10, "exposure_us": 156250})
        while self._running:
            while self._queue:
                cmd = self._queue.pop(0)
                if cmd[0] == "control":
                    self.controls[cmd[1]] = cmd[2]
                    self.settings_changed.emit({cmd[1]: cmd[2]})
                elif cmd[0] == "capture":
                    n, scale, fmt, folder, name = cmd[1:]
                    self.capture_started.emit(n, 0.1)
                    path = save_capture(frame, exposure=-6, gain=10, frames_averaged=n, scale=scale, fmt=fmt,
                                        folder=folder, name=name)
                    self.captures.append(str(path))
                    self.capture_done.emit(str(path))
            if not self._awaiting:
                self._awaiting = True
                self.frames_sent += 1
                self.frame_ready.emit(frame)
            time.sleep(1 / 30)

    def frame_consumed(self):
        self._awaiting = False

    def stop(self):
        self._running = False
        self.wait(3000)

    def set_control(self, name, value):
        self._queue.append(("control", name, value))

    def capture(self, n_frames, scale, fmt, folder, name=""):
        self._queue.append(("capture", n_frames, scale, fmt, folder, name))

    def set_flat_field(self, flat):
        self.flat = flat

    def __getattr__(self, name):  # recording, time-lapse: not simulated
        return lambda *a, **k: None
