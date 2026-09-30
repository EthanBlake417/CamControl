"""Video recording and time-lapse (no GUI code here; CameraWorker runs them).

Video: every camera frame goes to a video file. The camera's frame rate
varies (about 25 fps, less with long exposures), so frames are repeated or
skipped to match the file's fixed frame rate. That way the video plays back
at real speed.

Time-lapse: one capture (with averaging, like the Capture button) every
N seconds, saved as numbered images. Optionally the images are then
joined into a video that plays them back quickly.

Run directly for a self-test (writes to a temp folder):
    python -m camcontrol.recording
"""

import time
from dataclasses import dataclass, field
from pathlib import Path

import cv2
import numpy as np

from camcontrol.image_io import load_image_file
from camcontrol.processing.common import as_bgr

# Display name -> (file extension, OpenCV codec). MP4 is small and plays
# almost anywhere; AVI (Motion JPEG) keeps more detail but is much larger.
VIDEO_FORMATS = {
    "MP4": ("mp4", "mp4v"),
    "AVI (higher quality, large)": ("avi", "MJPG"),
}


def open_writer(path: Path, fmt: str, fps: float, size: tuple[int, int]) -> cv2.VideoWriter:
    _ext, codec = VIDEO_FORMATS[fmt]
    writer = cv2.VideoWriter(str(path), cv2.VideoWriter_fourcc(*codec), fps, size)
    if not writer.isOpened():
        raise RuntimeError(f"Could not start a {fmt} video at {path}")
    return writer


class VideoRecorder:
    """Writes live frames to a video file in real time."""

    def __init__(self, path: Path, fmt: str, fps: float, max_seconds: float = 0):
        self.path = Path(path)
        self.fmt = fmt
        self.fps = fps
        self.max_seconds = max_seconds  # 0 = until stopped
        self.start: float | None = None
        self.frames_written = 0
        self._writer: cv2.VideoWriter | None = None

    @property
    def elapsed(self) -> float:
        return 0.0 if self.start is None else time.perf_counter() - self.start

    @property
    def finished(self) -> bool:
        return bool(self.max_seconds) and self.elapsed >= self.max_seconds

    def add(self, frame: np.ndarray):
        frame = as_bgr(frame)
        if self._writer is None:  # size comes from the first frame
            h, w = frame.shape[:2]
            self._writer = open_writer(self.path, self.fmt, self.fps, (w, h))
            self.start = time.perf_counter()
        # Fill every output frame slot that has passed since the start with
        # this frame: repeats it if the camera is slow, skips if it's fast.
        due = int(self.elapsed * self.fps) + 1
        if self.max_seconds:
            due = min(due, round(self.max_seconds * self.fps))
        while self.frames_written < due:
            self._writer.write(frame)
            self.frames_written += 1

    def close(self):
        if self._writer is not None:
            self._writer.release()
            self._writer = None


@dataclass
class TimeLapse:
    """When to take each time-lapse image, and what's been taken."""
    interval: float           # seconds between images
    count: int = 0            # images to take; 0 = until stopped
    make_video: bool = False  # join the images into a video at the end
    video_fps: float = 10.0
    video_format: str = "MP4"
    start: float = field(default_factory=time.time)
    paths: list[str] = field(default_factory=list)

    @property
    def next_due(self) -> float:
        # Fixed schedule from the start, so small delays don't add up.
        return self.start + len(self.paths) * self.interval

    @property
    def done(self) -> bool:
        return bool(self.count) and len(self.paths) >= self.count

    def is_due(self) -> bool:
        return not self.done and time.time() >= self.next_due


def images_to_video(paths: list[str], out_path: Path, fmt: str, fps: float) -> Path:
    """Join images into a video, all at the first image's size."""
    writer = None
    size = None
    for p in paths:
        img = load_image_file(p)
        if img is None:
            continue
        img = as_bgr(img)
        if writer is None:
            size = (img.shape[1], img.shape[0])
            writer = open_writer(out_path, fmt, fps, size)
        elif (img.shape[1], img.shape[0]) != size:
            img = cv2.resize(img, size, interpolation=cv2.INTER_AREA)
        writer.write(img)
    if writer is None:
        raise RuntimeError("No readable images to make a video from.")
    writer.release()
    return out_path


if __name__ == "__main__":
    import tempfile

    from camcontrol.image_io import save_image_file

    folder = Path(tempfile.mkdtemp())
    frame = np.zeros((120, 160, 3), np.uint8)

    # 0.5 s of "camera" at ~12 fps, recorded at 20 fps -> about 10 frames.
    rec = VideoRecorder(folder / "test.mp4", "MP4", fps=20)
    t_end = time.perf_counter() + 0.5
    while time.perf_counter() < t_end:
        frame[:] = rec.frames_written * 20 % 255
        rec.add(frame)
        time.sleep(1 / 12)
    rec.close()
    got = cv2.VideoCapture(str(folder / "test.mp4")).get(cv2.CAP_PROP_FRAME_COUNT)
    print(f"recorded {rec.frames_written} frames in {rec.elapsed:.2f} s at 20 fps; file has {got:.0f}")
    assert 9 <= rec.frames_written <= 12 and got == rec.frames_written

    tl = TimeLapse(interval=10, count=2)
    assert tl.is_due()
    tl.paths.append("x")
    assert not tl.is_due() and abs(tl.next_due - tl.start - 10) < 1e-6

    paths = []
    for i in range(3):
        p = folder / f"tl-{i:03d}.png"
        save_image_file(p, np.full((90, 160, 3), i * 80, np.uint8))
        paths.append(str(p))
    out = images_to_video(paths, folder / "tl.avi", "AVI (higher quality, large)", 5)
    print("time-lapse video frames:", cv2.VideoCapture(str(out)).get(cv2.CAP_PROP_FRAME_COUNT))
    print("recording OK")
