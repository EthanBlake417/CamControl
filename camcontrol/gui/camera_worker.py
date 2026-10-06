"""Runs the camera in a background thread so the GUI never freezes.

All camera access happens on this one thread (OpenCV's VideoCapture isn't
safe to use from two threads at once). The GUI asks for changes by calling
set_control() / capture() / start_recording() etc., which just put a
command on a queue. The thread applies commands between frames.

Frames are sent to the GUI one at a time: a new frame is only emitted after
the GUI calls frame_consumed(). If the GUI falls behind, frames are dropped
instead of piling up in memory. Video recording gets every frame, whether
or not the GUI keeps up.

With a flat-field set (set_flat_field), live frames, captures, videos and
time-lapse images are corrected here, on this thread, before anything else.
"""

import queue
import time
from pathlib import Path

from PySide6.QtCore import QThread, Signal

from camcontrol.camera import Camera, exposure_seconds
from camcontrol.capture import grab_average, next_capture_path, save_capture
from camcontrol.recording import VIDEO_FORMATS, TimeLapse, VideoRecorder, images_to_video

PROGRESS_EVERY = 0.5  # seconds between recording progress signals


class CameraWorker(QThread):
    opened = Signal(dict)                    # name, size, modes, ranges ({control name: range dict or None})
    open_failed = Signal(str)                # camera not found / couldn't open (thread ends)
    frame_ready = Signal(object)             # numpy BGR frame
    settings_changed = Signal(dict)          # {control name: current value}
    capture_started = Signal(int, float)     # frames to average, estimated seconds
    capture_done = Signal(str)               # saved image path
    error = Signal(str)
    recording_changed = Signal(bool, str)    # recording now?, video path
    recording_progress = Signal(float, int)  # seconds recorded, frames written
    timelapse_progress = Signal(int, int, float, str)  # taken, total (0 = no limit), next due (epoch s), last path
    timelapse_finished = Signal(str)         # summary message

    def __init__(self, index: int | None = 0, size: tuple[int, int] | None = None, parent=None):
        """size: frame size to ask the camera for; None = its largest.
        index None: the camera isn't plugged in; the thread just reports that."""
        super().__init__(parent)
        self._index = index
        self._size = size
        self._commands: queue.Queue = queue.Queue()
        self._running = True
        self._awaiting_ack = False
        self._flat = None
        self._recorder: VideoRecorder | None = None
        self._last_progress = 0.0
        self._timelapse: TimeLapse | None = None
        self._timelapse_opts: dict = {}

    # --- called from the GUI thread ------------------------------------------

    def set_control(self, name: str, value: float):
        self._commands.put(("control", name, value))

    def capture(self, n_frames: int, scale: float, fmt: str, folder: Path, name: str = ""):
        self._commands.put(("capture", n_frames, scale, fmt, folder, name))

    def set_flat_field(self, flat):
        """A processing.flatfield.FlatField to apply, or None for raw frames."""
        self._commands.put(("flat", flat))

    def start_recording(self, folder: Path, name: str, video_format: str, fps: float, max_seconds: float):
        self._commands.put(("record", folder, name, video_format, fps, max_seconds))

    def stop_recording(self):
        self._commands.put(("record_stop",))

    def start_timelapse(self, timelapse: TimeLapse, capture_opts: dict):
        """capture_opts: n_frames, scale, fmt, folder, name (as for capture())."""
        self._commands.put(("timelapse", timelapse, capture_opts))

    def stop_timelapse(self):
        self._commands.put(("timelapse_stop",))

    def frame_consumed(self):
        self._awaiting_ack = False

    def stop(self):
        self._running = False
        # A long exposure can block a read for ~1.25 s, so allow a few seconds.
        self.wait(5000)

    # --- the thread ----------------------------------------------------------

    def run(self):
        if self._index is None:
            self.open_failed.emit("Camera not found.")
            return
        try:
            cam = Camera(self._index, self._size)
        except Exception as e:  # not plugged in, off, or in use by another program
            self.open_failed.emit(str(e))
            return

        self.opened.emit({"name": cam.name, "size": cam.size, "modes": cam.modes, "ranges": cam.ranges})
        self.settings_changed.emit(cam.values())
        try:
            while self._running:
                self._handle_commands(cam)
                if self._timelapse is not None and self._timelapse.is_due():
                    self._timelapse_capture(cam)
                frame = cam.read()
                if frame is None:
                    continue
                if self._flat is not None:
                    frame = self._flat.apply(frame)
                if self._recorder is not None:
                    self._record(frame)
                if not self._awaiting_ack:
                    self._awaiting_ack = True
                    self.frame_ready.emit(frame)
        finally:
            # Closing the app mid-recording still leaves a playable file.
            self._stop_recording()
            self._stop_timelapse("Time-lapse stopped (app closed).")
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
            kind = cmd[0]
            if kind == "capture":
                captures.append(cmd)
            elif kind == "flat":
                self._flat = cmd[1]
            elif kind == "record":
                self._start_recording(*cmd[1:])
            elif kind == "record_stop":
                self._stop_recording()
            elif kind == "timelapse":
                self._stop_timelapse(None)
                self._timelapse, self._timelapse_opts = cmd[1], cmd[2]
            elif kind == "timelapse_stop":
                self._stop_timelapse("Time-lapse stopped.")
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

        for _, n_frames, scale, fmt, folder, name in captures:
            self._capture(cam, n_frames, scale, fmt, folder, name)

    # --- still capture -----------------------------------------------------------

    def _capture(self, cam: Camera, n_frames, scale, fmt, folder, name,
                 extra: dict | None = None, prefix: str = "cap", announce: bool = True) -> str | None:
        """Average, correct and save one image. Returns its path, or None on failure."""
        if announce:
            exposure = cam.exposure
            estimate = n_frames * max(exposure_seconds(exposure) if exposure is not None else 0, 1 / 25)
            self.capture_started.emit(n_frames, estimate)
        try:
            image, got = grab_average(cam, n_frames)
            extra = dict(extra or {})
            if cam.frames_summed > 1:
                extra["frames_summed_per_exposure"] = cam.frames_summed
            if self._flat is not None:
                image = self._flat.apply(image)
                extra["flat_field"] = self._flat.name
            path = save_capture(
                image,
                exposure=cam.exposure,
                gain=cam.gain,
                frames_averaged=got,
                scale=scale,
                fmt=fmt,
                folder=folder,
                name=name,
                prefix=prefix,
                extra=extra,
            )
        except Exception as e:  # report any failure to the GUI instead of dying
            self.error.emit(f"Capture failed: {e}")
            return None
        if announce:
            self.capture_done.emit(str(path))
        return str(path)

    # --- video -------------------------------------------------------------------

    def _start_recording(self, folder, name, video_format, fps, max_seconds):
        self._stop_recording()
        ext, _codec = VIDEO_FORMATS[video_format]
        folder = Path(folder)
        folder.mkdir(parents=True, exist_ok=True)
        path = next_capture_path(folder, name, ext, prefix="vid")
        self._recorder = VideoRecorder(path, video_format, fps, max_seconds)
        self._last_progress = 0.0
        self.recording_changed.emit(True, str(path))

    def _record(self, frame):
        rec = self._recorder
        try:
            rec.add(frame)
        except Exception as e:
            self.error.emit(f"Recording failed: {e}")
            self._stop_recording()
            return
        now = time.perf_counter()
        if now - self._last_progress >= PROGRESS_EVERY:
            self._last_progress = now
            self.recording_progress.emit(rec.elapsed, rec.frames_written)
        if rec.finished:
            self._stop_recording()

    def _stop_recording(self):
        rec = self._recorder
        if rec is None:
            return
        self._recorder = None
        rec.close()
        self.recording_progress.emit(rec.elapsed, rec.frames_written)
        self.recording_changed.emit(False, str(rec.path) if rec.frames_written else "")

    # --- time-lapse ----------------------------------------------------------------

    def _timelapse_capture(self, cam: Camera):
        tl = self._timelapse
        o = self._timelapse_opts
        index = len(tl.paths) + 1
        path = self._capture(cam, o["n_frames"], o["scale"], o["fmt"], o["folder"], o["name"],
                             extra={"timelapse": {"index": index, "interval_s": tl.interval}},
                             prefix="tl", announce=False)
        if path is None:  # already reported; stop instead of failing every interval
            self._stop_timelapse("Time-lapse stopped after an error.")
            return
        tl.paths.append(path)
        self.timelapse_progress.emit(len(tl.paths), tl.count, tl.next_due, path)
        if tl.done:
            self._stop_timelapse(None)

    def _stop_timelapse(self, reason: str | None):
        tl = self._timelapse
        if tl is None:
            return
        self._timelapse = None
        message = reason or "Time-lapse finished."
        message += f" {len(tl.paths)} image{'s' if len(tl.paths) != 1 else ''} saved."
        if tl.make_video and len(tl.paths) >= 2:
            try:
                ext, _codec = VIDEO_FORMATS[tl.video_format]
                out = next_capture_path(Path(self._timelapse_opts["folder"]), self._timelapse_opts["name"],
                                        ext, prefix="timelapse")
                images_to_video(tl.paths, out, tl.video_format, tl.video_fps)
                message += f" Video: {out.name}"
            except Exception as e:
                message += f" The video couldn't be made: {e}"
        self.timelapse_finished.emit(message)
