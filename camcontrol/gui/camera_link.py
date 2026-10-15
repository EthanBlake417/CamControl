"""One camera in use by a view: its background worker and what it has told
us (frame size, sizes it offers, control ranges and values, frame rate).

Several can be open at once, one per camera (each on its own thread). Their
signals are the worker's, with the connection itself first, so the main
window knows which camera (and so which view) they're about.
"""

import time
from collections import deque

from PySide6.QtCore import QObject, Signal

from camcontrol.gui.camera_worker import CameraWorker


class CameraLink(QObject):
    opened = Signal(object, dict)                 # link, info (see CameraWorker.opened)
    open_failed = Signal(object, str)
    frame_ready = Signal(object, object)          # link, frame
    settings_changed = Signal(object, dict)
    capture_started = Signal(object, int, float)
    capture_done = Signal(object, str)
    error = Signal(object, str)
    recording_changed = Signal(object, bool, str)
    recording_progress = Signal(object, float, int)
    timelapse_progress = Signal(object, int, int, float, str)
    timelapse_finished = Signal(object, str)

    def __init__(self, key: tuple[str, int] | None, parent=None):
        """key: (name, n) as camera_list() gives; None: no camera chosen yet."""
        super().__init__(parent)
        self.key = key
        self.worker: CameraWorker | None = None
        self.open = False
        self.name = ""
        self.frame_size: tuple[int, int] | None = None
        self.modes: list[dict] = []
        self.ranges: dict = {}
        self.values: dict = {}           # latest control values the camera reported
        self.pending: dict | None = None  # values to set once it opens (e.g. from a settings file)
        self.status = ("Connecting...", "", "")  # text, style, tooltip
        self.recording = False
        self.timelapse = False
        self._frame_times: deque[float] = deque(maxlen=30)

    def start(self, index: int | None, size: tuple[int, int] | None, flat=None):
        """Open camera number index (None: not plugged in, the worker just says so)."""
        self.stop()
        self.open = False
        self.status = ("Connecting...", "", "")
        self._frame_times.clear()
        w = CameraWorker(index, size)

        def current(handler):
            """Only while w is this camera's worker: a frame (or anything else)
            still queued from a worker closed since is dropped."""
            return lambda *args: handler(*args) if self.worker is w else None
        w.opened.connect(current(self._on_opened))
        w.open_failed.connect(current(self._on_open_failed))
        w.frame_ready.connect(current(self._on_frame))
        w.settings_changed.connect(current(self._on_settings))
        w.capture_started.connect(current(lambda n, s: self.capture_started.emit(self, n, s)))
        w.capture_done.connect(current(lambda p: self.capture_done.emit(self, p)))
        w.error.connect(current(lambda m: self.error.emit(self, m)))
        w.recording_changed.connect(current(self._on_recording))
        w.recording_progress.connect(current(lambda s, f: self.recording_progress.emit(self, s, f)))
        w.timelapse_progress.connect(current(lambda *a: self.timelapse_progress.emit(self, *a)))
        w.timelapse_finished.connect(current(self._on_timelapse_finished))
        if flat is not None:
            w.set_flat_field(flat)
        self.worker = w
        w.start()

    def stop(self):
        """Close the camera (a recording is finished and saved)."""
        if self.worker is not None:
            self.worker.stop()
            self.worker = None
        self.open = False
        self.recording = self.timelapse = False

    def fps(self) -> float | None:
        if len(self._frame_times) > 1:
            span = self._frame_times[-1] - self._frame_times[0]
            if span > 0:
                return (len(self._frame_times) - 1) / span
        return None

    # --- from the worker -----------------------------------------------------------------

    def _on_opened(self, info: dict):
        self.open = True
        self.name = info["name"]
        self.frame_size = tuple(info["size"])
        self.modes = info["modes"]
        self.ranges = info["ranges"]
        self.values = {}
        self.status = ("Connected", "color: green;", info["name"])
        self.opened.emit(self, info)

    def _on_open_failed(self, message: str):
        self.open = False
        self.frame_size = None
        self.modes = []
        self.status = ("Not connected", "color: #c00; font-weight: bold;", message)
        self.open_failed.emit(self, message)

    def _on_frame(self, frame):
        self.worker.frame_consumed()  # always, so the worker sends the next one
        self._frame_times.append(time.perf_counter())
        self.frame_ready.emit(self, frame)

    def _on_settings(self, values: dict):
        self.values.update(values)
        self.settings_changed.emit(self, values)

    def _on_recording(self, on: bool, path: str):
        self.recording = on
        self.recording_changed.emit(self, on, path)

    def _on_timelapse_finished(self, message: str):
        self.timelapse = False
        self.timelapse_finished.emit(self, message)
