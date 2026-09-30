"""Video and time-lapse controls, shown as the Video and Time-lapse tabs of
the Controls panel (next to Photo).

This only collects settings and shows progress. The main window passes the
requests to the CameraWorker, which does the recording, and feeds its
progress signals back here. Folder, name and flat-field come from the shared
Output box; time-lapse images use the Photo tab's averaging, size and format.
"""

import time
from pathlib import Path

from PySide6.QtCore import QObject, QTimer, Signal
from PySide6.QtWidgets import (
    QCheckBox,
    QComboBox,
    QDoubleSpinBox,
    QFormLayout,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QSpinBox,
    QWidget,
)

from camcontrol.recording import VIDEO_FORMATS, TimeLapse

UNITS = [("seconds", 1), ("minutes", 60), ("hours", 3600)]
BIG_BUTTON_HEIGHT = 40  # same as the Capture button


def format_duration(seconds: float) -> str:
    seconds = max(0, round(seconds))
    h, rest = divmod(seconds, 3600)
    m, s = divmod(rest, 60)
    return f"{h}:{m:02d}:{s:02d}" if h else f"{m}:{s:02d}"


class RecordPanel(QObject):
    """Builds two pages, video_page and timelapse_page, for the main window to place."""
    record_requested = Signal(bool)      # start (True) / stop (False) video
    timelapse_requested = Signal(bool)   # start / stop time-lapse

    def __init__(self, parent=None):
        super().__init__(parent)

        # --- video ---
        self.video_page = QWidget()
        form = QFormLayout(self.video_page)
        self.video_format = QComboBox()
        self.video_format.addItems(list(VIDEO_FORMATS))
        form.addRow("Format", self.video_format)
        self.video_fps = QSpinBox()
        self.video_fps.setRange(1, 60)
        self.video_fps.setValue(25)
        self.video_fps.setSuffix(" fps")
        self.video_fps.setToolTip("Playback frame rate. The video plays at real speed whatever the\n"
                                  "camera manages (about 25 fps, less with long exposures).")
        form.addRow("Frame rate", self.video_fps)
        self.video_limit = QSpinBox()
        self.video_limit.setRange(0, 24 * 3600)
        self.video_limit.setSuffix(" s")
        self.video_limit.setSpecialValueText("no limit")
        form.addRow("Stop after", self.video_limit)
        self.record_button = QPushButton("Record")
        self.record_button.setCheckable(True)
        self.record_button.setMinimumHeight(BIG_BUTTON_HEIGHT)
        self.record_button.clicked.connect(self.record_requested)
        form.addRow(self.record_button)
        self.record_status = QLabel("")
        self.record_status.setWordWrap(True)
        form.addRow(self.record_status)

        # --- time-lapse ---
        self.timelapse_page = QWidget()
        form = QFormLayout(self.timelapse_page)
        every = QHBoxLayout()
        self.tl_interval = QDoubleSpinBox()
        self.tl_interval.setRange(0.5, 100000)
        self.tl_interval.setDecimals(1)
        self.tl_interval.setValue(10)
        self.tl_unit = QComboBox()
        for text, factor in UNITS:
            self.tl_unit.addItem(text, factor)
        every.addWidget(self.tl_interval, stretch=1)
        every.addWidget(self.tl_unit)
        form.addRow("Every", every)
        count_row = QHBoxLayout()
        self.tl_count = QSpinBox()
        self.tl_count.setRange(0, 100000)
        self.tl_count.setValue(60)
        self.tl_count.setSpecialValueText("until stopped")
        self.tl_count.setSuffix(" images")
        count_row.addWidget(self.tl_count, stretch=1)
        self.tl_total = QLabel()
        self.tl_total.setStyleSheet("color: gray;")
        count_row.addWidget(self.tl_total)
        form.addRow("Take", count_row)

        video_row = QHBoxLayout()
        self.tl_video = QCheckBox("Make a video at")
        self.tl_video.setChecked(True)
        self.tl_video_fps = QSpinBox()
        self.tl_video_fps.setRange(1, 60)
        self.tl_video_fps.setValue(10)
        self.tl_video_fps.setSuffix(" fps")
        self.tl_video.setToolTip("Join the images into a video at the end (format from the Video tab).")
        video_row.addWidget(self.tl_video)
        video_row.addWidget(self.tl_video_fps)
        video_row.addStretch()
        form.addRow(video_row)

        self.tl_button = QPushButton("Start time-lapse")
        self.tl_button.setCheckable(True)
        self.tl_button.setMinimumHeight(BIG_BUTTON_HEIGHT)
        self.tl_button.setToolTip("Each image uses the Photo tab's averaging, size and format.")
        self.tl_button.clicked.connect(self.timelapse_requested)
        form.addRow(self.tl_button)
        self.tl_status = QLabel("Images use the Photo tab's averaging, size and format.")
        self.tl_status.setWordWrap(True)
        form.addRow(self.tl_status)

        for w in (self.tl_interval, self.tl_count, self.tl_video_fps):
            w.valueChanged.connect(self._update_total)
        self.tl_unit.currentIndexChanged.connect(self._update_total)
        self.tl_video.toggled.connect(self._update_total)
        self._update_total()

        # Counts down to the next time-lapse image.
        self._tl_info: tuple[int, int, float] | None = None  # taken, total, next due
        self._tick = QTimer(self)
        self._tick.setInterval(1000)
        self._tick.timeout.connect(self._show_timelapse_status)

    def set_enabled(self, on: bool):
        """Off while the camera isn't connected."""
        self.video_page.setEnabled(on)
        self.timelapse_page.setEnabled(on)

    # --- settings ---------------------------------------------------------------------

    @property
    def interval_seconds(self) -> float:
        return self.tl_interval.value() * self.tl_unit.currentData()

    def video_settings(self) -> dict:
        return {"video_format": self.video_format.currentText(), "fps": self.video_fps.value(),
                "max_seconds": self.video_limit.value()}

    def make_timelapse(self) -> TimeLapse:
        return TimeLapse(interval=self.interval_seconds, count=self.tl_count.value(),
                         make_video=self.tl_video.isChecked(), video_fps=self.tl_video_fps.value(),
                         video_format=self.video_format.currentText())

    def state(self) -> dict:
        """For saving in QSettings and settings files."""
        return {"video_format": self.video_format.currentText(), "video_fps": self.video_fps.value(),
                "video_limit": self.video_limit.value(), "tl_interval": self.tl_interval.value(),
                "tl_unit": self.tl_unit.currentIndex(), "tl_count": self.tl_count.value(),
                "tl_video": self.tl_video.isChecked(), "tl_video_fps": self.tl_video_fps.value()}

    def set_state(self, s: dict):
        if s.get("video_format") in VIDEO_FORMATS:
            self.video_format.setCurrentText(s["video_format"])
        for key, widget in (("video_fps", self.video_fps), ("video_limit", self.video_limit),
                            ("tl_interval", self.tl_interval), ("tl_count", self.tl_count),
                            ("tl_video_fps", self.tl_video_fps)):
            if key in s:
                widget.setValue(type(widget.value())(s[key]))
        if "tl_unit" in s:
            self.tl_unit.setCurrentIndex(int(s["tl_unit"]))
        if "tl_video" in s:
            self.tl_video.setChecked(bool(s["tl_video"]))

    def _update_total(self):
        n = self.tl_count.value()
        if not n:
            self.tl_total.setText("")
            self.tl_total.setToolTip("")
            return
        self.tl_total.setText(f"= {format_duration((n - 1) * self.interval_seconds)}")
        tip = "How long the time-lapse takes."
        if self.tl_video.isChecked():
            tip += f" The video will last {n / self.tl_video_fps.value():.1f} s."
        self.tl_total.setToolTip(tip)

    # --- progress from the camera worker -----------------------------------------------

    def set_recording(self, on: bool, path: str):
        self.record_button.setChecked(on)
        self.record_button.setText("Stop recording" if on else "Record")
        self.record_button.setStyleSheet("color: red; font-weight: bold;" if on else "")
        if on:
            self.record_status.setText(f"Recording to {Path(path).name}")
        elif path:
            self.record_status.setText(f"Saved {Path(path).name}")
            self.record_status.setToolTip(path)
        for w in (self.video_format, self.video_fps, self.video_limit):
            w.setEnabled(not on)

    def set_record_progress(self, seconds: float, frames: int):
        if self.record_button.isChecked():
            self.record_status.setText(f"Recording {format_duration(seconds)} ({frames} frames)")

    def set_timelapse_running(self, on: bool):
        self.tl_button.setChecked(on)
        self.tl_button.setText("Stop time-lapse" if on else "Start time-lapse")
        self.tl_button.setStyleSheet("color: red; font-weight: bold;" if on else "")
        for w in (self.tl_interval, self.tl_unit, self.tl_count, self.tl_video, self.tl_video_fps):
            w.setEnabled(not on)
        if on:
            self._tl_info = (0, self.tl_count.value(), time.time())
            self.tl_status.setText("Starting...")
            self._tick.start()
        else:
            self._tl_info = None
            self._tick.stop()

    def set_timelapse_progress(self, taken: int, total: int, next_due: float):
        self._tl_info = (taken, total, next_due)
        self._show_timelapse_status()

    def set_timelapse_message(self, message: str):
        self.set_timelapse_running(False)
        self.tl_status.setText(message)

    def _show_timelapse_status(self):
        if self._tl_info is None:
            return
        taken, total, next_due = self._tl_info
        of = f" of {total}" if total else ""
        if total and taken >= total:
            self.tl_status.setText(f"{taken}{of} images taken. Finishing...")
            return
        self.tl_status.setText(f"{taken}{of} images taken. Next in {format_duration(next_due - time.time())}")
