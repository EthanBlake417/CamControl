"""Runs the camera in a background thread so the GUI never freezes.

All camera access happens on this one thread (OpenCV's VideoCapture isn't
safe to use from two threads at once). The GUI asks for changes by calling
set_control() / capture(), which just put a command on a queue. The thread applies commands between frames.

Frames are sent to the GUI one at a time: a new frame is only emitted after
the GUI calls frame_consumed(). If the GUI falls behind, frames are dropped
instead of piling up in memory.
"""

import queue
from pathlib import Path

from PySide6.QtCore import QThread, Signal

from camcontrol.camera import Camera, exposure_seconds
from camcontrol.capture import grab_average, save_capture


class CameraWorker(QThread):
    opened = Signal(dict)                    # {control name: range dict}
    frame_ready = Signal(object)             # numpy BGR frame
    settings_changed = Signal(dict)          # {control name: current value}
    capture_started = Signal(int, float)     # frames to average, estimated seconds
    capture_done = Signal(str)               # saved image path
    error = Signal(str)

    def __init__(self, index: int = 0, parent=None):
        super().__init__(parent)
        self._index = index
        self._commands: queue.Queue = queue.Queue()
        self._running = True
        self._awaiting_ack = False

    # --- called from the GUI thread ------------------------------------------

    def set_control(self, name: str, value: float):
        self._commands.put(("control", name, value))

    def capture(self, n_frames: int, save_size: tuple[int, int], fmt: str, folder: Path):
        self._commands.put(("capture", n_frames, save_size, fmt, folder))

    def frame_consumed(self):
        self._awaiting_ack = False

    def stop(self):
        self._running = False
        # A long exposure can block a read for ~1.25 s, so allow a few seconds.
        self.wait(5000)

    # --- the thread ----------------------------------------------------------

    def run(self):
        try:
            cam = Camera(self._index)
        except RuntimeError as e:
            self.error.emit(str(e))
            return

        self.opened.emit(cam.ranges)
        self.settings_changed.emit(cam.values())
        try:
            while self._running:
                self._handle_commands(cam)
                frame = cam.read()
                if frame is not None and not self._awaiting_ack:
                    self._awaiting_ack = True
                    self.frame_ready.emit(frame)
        finally:
            cam.close()

    def _handle_commands(self, cam: Camera):
        # Take everything queued. For each control only the latest value
        # matters (dragging a slider queues many), so earlier ones are skipped.
        pending = []
        while True:
            try:
                pending.append(self._commands.get_nowait())
            except queue.Empty:
                break

        latest = {}
        captures = []
        for cmd in pending:
            if cmd[0] == "capture":
                captures.append(cmd)
            else:
                _, name, value = cmd
                latest[name] = value

        for name, value in latest.items():
            try:
                cam.set(name, value)
            except Exception as e:  # e.g. the camera rejects a value
                self.error.emit(f"Could not set {name}: {e}")
        if latest:
            self.settings_changed.emit(cam.values())

        for _, n_frames, save_size, fmt, folder in captures:
            self._capture(cam, n_frames, save_size, fmt, folder)

    def _capture(self, cam: Camera, n_frames, save_size, fmt, folder):
        estimate = n_frames * max(exposure_seconds(cam.exposure), 1 / 25)
        self.capture_started.emit(n_frames, estimate)
        try:
            image, got = grab_average(cam, n_frames)
            path = save_capture(
                image,
                exposure=cam.exposure,
                gain=cam.gain,
                frames_averaged=got,
                save_size=save_size,
                fmt=fmt,
                folder=folder,
            )
        except Exception as e:  # report any failure to the GUI instead of dying
            self.error.emit(f"Capture failed: {e}")
            return
        self.capture_done.emit(str(path))
