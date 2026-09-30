"""Video playback: a bar under the image with play/pause, frame steps and a
seek slider. Frames go to the main image view, so a paused frame can be
zoomed, measured, counted or saved (File > Save image as) like any image.

Frames are decoded with OpenCV on the GUI thread; a 1080p frame takes a few
milliseconds, well within one frame's time.
"""

import cv2
from PySide6.QtCore import Qt, QTimer, Signal
from PySide6.QtWidgets import QHBoxLayout, QLabel, QPushButton, QSlider, QStyle, QToolButton, QWidget

from camcontrol.gui.record_panel import format_duration


class VideoPlayer(QWidget):
    frame_ready = Signal(object)  # numpy BGR frame to show
    close_requested = Signal()

    def __init__(self, parent=None):
        super().__init__(parent)
        self.path: str | None = None
        self._cap: cv2.VideoCapture | None = None
        self.fps = 25.0
        self.frame_count = 0
        self.position = 0  # index of the frame on screen

        layout = QHBoxLayout(self)
        layout.setContentsMargins(4, 2, 4, 2)
        style = self.style()

        def button(icon, tip, slot):
            b = QToolButton()
            b.setIcon(style.standardIcon(icon))
            b.setToolTip(tip)
            b.setAutoRaise(True)
            b.clicked.connect(slot)
            layout.addWidget(b)
            return b

        self.play_button = button(QStyle.StandardPixmap.SP_MediaPlay, "Play / pause", self.toggle_play)
        button(QStyle.StandardPixmap.SP_MediaSeekBackward, "Previous frame", lambda: self.step(-1))
        button(QStyle.StandardPixmap.SP_MediaSeekForward, "Next frame", lambda: self.step(1))
        self.slider = QSlider(Qt.Orientation.Horizontal)
        self.slider.setToolTip("Drag to jump to a point in the video.")
        self.slider.valueChanged.connect(self._on_slider)
        layout.addWidget(self.slider, stretch=1)
        self.time_label = QLabel()
        self.time_label.setMinimumWidth(170)
        layout.addWidget(self.time_label)
        close = QPushButton("Close video")
        close.clicked.connect(self.close_requested)
        layout.addWidget(close)

        self._timer = QTimer(self)
        self._timer.timeout.connect(self._next_frame)

    # --- opening ------------------------------------------------------------------

    def open(self, path: str) -> bool:
        """Open a video and show its first frame (paused)."""
        self.close_video()
        cap = cv2.VideoCapture(str(path))
        if not cap.isOpened():
            return False
        self._cap = cap
        self.path = str(path)
        fps = cap.get(cv2.CAP_PROP_FPS)
        self.fps = fps if 1 <= fps <= 240 else 25.0
        self.frame_count = max(1, int(cap.get(cv2.CAP_PROP_FRAME_COUNT)))
        self.slider.blockSignals(True)
        self.slider.setRange(0, self.frame_count - 1)
        self.slider.setValue(0)
        self.slider.blockSignals(False)
        self._timer.setInterval(round(1000 / self.fps))
        self.position = -1
        if not self._read():
            self.close_video()
            return False
        return True

    def close_video(self):
        self.pause()
        if self._cap is not None:
            self._cap.release()
        self._cap = None
        self.path = None

    @property
    def is_open(self) -> bool:
        return self._cap is not None

    @property
    def playing(self) -> bool:
        return self._timer.isActive()

    # --- playback -------------------------------------------------------------------

    def toggle_play(self):
        if self.playing:
            self.pause()
        else:
            self.play()

    def play(self):
        if self._cap is None:
            return
        if self.position >= self.frame_count - 1:
            self.seek(0)  # at the end: start again
        self._timer.start()
        self.play_button.setIcon(self.style().standardIcon(QStyle.StandardPixmap.SP_MediaPause))

    def pause(self):
        self._timer.stop()
        self.play_button.setIcon(self.style().standardIcon(QStyle.StandardPixmap.SP_MediaPlay))

    def step(self, delta: int):
        self.pause()
        self.seek(self.position + delta)

    def seek(self, index: int):
        if self._cap is None:
            return
        index = min(max(index, 0), self.frame_count - 1)
        self._cap.set(cv2.CAP_PROP_POS_FRAMES, index)
        self.position = index - 1
        self._read()

    def _on_slider(self, value: int):
        if value != self.position:
            self.seek(value)

    def _next_frame(self):
        if not self._read():
            self.pause()  # end of the video

    def _read(self) -> bool:
        ok, frame = self._cap.read()
        if not ok:
            # Frame counts in file headers can be a little high; stop at the real end.
            self.frame_count = max(1, self.position + 1)
            self.slider.setMaximum(self.frame_count - 1)
            return False
        self.position += 1
        self.slider.blockSignals(True)
        self.slider.setValue(self.position)
        self.slider.blockSignals(False)
        self.time_label.setText(
            f"{format_duration(self.position / self.fps)} / {format_duration(self.frame_count / self.fps)}"
            f"   frame {self.position + 1} of {self.frame_count}")
        self.frame_ready.emit(frame)
        return True

