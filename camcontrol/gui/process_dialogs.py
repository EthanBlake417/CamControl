"""Dialogs for the processing tools that combine several images.

Each dialog collects a list of image files and the tool's options. The
main window then loads the files and calls process() in a background
thread, so process() must only use its arguments, never the widgets.
"""

from pathlib import Path

import numpy as np
from PySide6.QtCore import Qt
from PySide6.QtWidgets import (
    QAbstractItemView,
    QCheckBox,
    QComboBox,
    QDialog,
    QDialogButtonBox,
    QDoubleSpinBox,
    QFileDialog,
    QFormLayout,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QMessageBox,
    QPushButton,
    QTableWidget,
    QTableWidgetItem,
    QVBoxLayout,
    QWidget,
)

from camcontrol.image_io import IMAGE_EXTENSIONS
from camcontrol.processing.fluorescence import COLORS, Channel, composite, guess_color
from camcontrol.processing.focus_stack import focus_stack
from camcontrol.processing.hdr import exposure_fusion
from camcontrol.processing.stitch import stitch

PATH_ROLE = Qt.ItemDataRole.UserRole
FILE_FILTER = "Images ({});;All files (*)".format(" ".join(f"*{e}" for e in sorted(IMAGE_EXTENSIONS)))


class ImageSetDialog(QDialog):
    title = ""
    short_name = ""       # used for the result's file name, e.g. "stack-001.tif"
    help_text = ""
    min_images = 2
    extra_columns: list[str] = []

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setWindowTitle(self.title)
        self.start_folder = ""
        self.resize(560, 460)
        layout = QVBoxLayout(self)

        help_label = QLabel(self.help_text)
        help_label.setWordWrap(True)
        layout.addWidget(help_label)

        self.table = QTableWidget(0, 1 + len(self.extra_columns))
        self.table.setHorizontalHeaderLabels(["Image"] + self.extra_columns)
        self.table.setSelectionBehavior(QAbstractItemView.SelectionBehavior.SelectRows)
        self.table.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers)
        self.table.verticalHeader().setVisible(False)
        header = self.table.horizontalHeader()
        header.setSectionResizeMode(QHeaderView.ResizeMode.ResizeToContents)
        header.setSectionResizeMode(0, QHeaderView.ResizeMode.Stretch)
        layout.addWidget(self.table, stretch=1)

        buttons = QHBoxLayout()
        add = QPushButton("Add images...")
        add.clicked.connect(self._add_files)
        remove = QPushButton("Remove")
        remove.clicked.connect(self._remove_selected)
        clear = QPushButton("Clear")
        clear.clicked.connect(lambda: self.table.setRowCount(0))
        for b in (add, remove, clear):
            buttons.addWidget(b)
        buttons.addStretch()
        layout.addLayout(buttons)

        self.options = QFormLayout()
        layout.addLayout(self.options)

        box = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel)
        box.button(QDialogButtonBox.StandardButton.Ok).setText("Run")
        box.accepted.connect(self.accept)
        box.rejected.connect(self.reject)
        layout.addWidget(box)

    # --- the file list -------------------------------------------------------------

    def set_paths(self, paths: list[str]):
        self.table.setRowCount(0)
        for p in paths:
            self._add_row(p)

    def paths(self) -> list[str]:
        return [self.table.item(r, 0).data(PATH_ROLE) for r in range(self.table.rowCount())]

    def _add_row(self, path: str):
        row = self.table.rowCount()
        self.table.insertRow(row)
        item = QTableWidgetItem(Path(path).name)
        item.setData(PATH_ROLE, path)
        item.setToolTip(path)
        self.table.setItem(row, 0, item)
        for col, widget in enumerate(self.row_widgets(path), start=1):
            self.table.setCellWidget(row, col, widget)

    def row_widgets(self, path: str) -> list[QWidget]:
        """Per-image option widgets, one per extra column."""
        return []

    def _add_files(self):
        paths, _ = QFileDialog.getOpenFileNames(self, "Add images", self.start_folder, FILE_FILTER)
        for p in paths:
            self._add_row(p)

    def _remove_selected(self):
        for r in sorted({i.row() for i in self.table.selectedIndexes()}, reverse=True):
            self.table.removeRow(r)

    def accept(self):
        n = self.table.rowCount()
        if n < self.min_images:
            QMessageBox.information(self, self.title,
                                    f"Add at least {self.min_images} image{'s' if self.min_images > 1 else ''}.")
            return
        super().accept()

    # --- to override -------------------------------------------------------------------

    def settings(self) -> dict:
        """The chosen options (read on the GUI thread, saved with the result)."""
        return {}

    @staticmethod
    def process(images: list[np.ndarray], settings: dict) -> np.ndarray:
        raise NotImplementedError


class FocusStackDialog(ImageSetDialog):
    title = "Focus stack"
    short_name = "stack"
    help_text = ("Combines the sharp parts of images focused at different depths. "
                 "Take one image at each focus step through the sample, without moving it.")

    def __init__(self, parent=None):
        super().__init__(parent)
        self.align_check = QCheckBox("Align images first (refocusing shifts the image slightly)")
        self.align_check.setChecked(True)
        self.options.addRow(self.align_check)

    def settings(self):
        return {"align": self.align_check.isChecked()}

    @staticmethod
    def process(images, settings):
        result, _depth = focus_stack(images, align=settings["align"])
        return result


class HdrDialog(ImageSetDialog):
    title = "HDR (exposure fusion)"
    short_name = "hdr"
    help_text = ("Blends images of the same view taken at different exposures, so bright and "
                 "dark areas both show detail. Use 3 or more exposures, e.g. short, medium and long.")

    def __init__(self, parent=None):
        super().__init__(parent)
        self.align_check = QCheckBox("Align images first")
        self.align_check.setChecked(True)
        self.options.addRow(self.align_check)

    def settings(self):
        return {"align": self.align_check.isChecked()}

    @staticmethod
    def process(images, settings):
        return exposure_fusion(images, align=settings["align"])


class StitchDialog(ImageSetDialog):
    title = "Stitch"
    short_name = "stitch"
    help_text = ("Joins overlapping images into one larger image. Move the sample between shots "
                 "so neighbouring images overlap by about a third. Large sets can take a while.")

    def __init__(self, parent=None):
        super().__init__(parent)
        self.mode_combo = QComboBox()
        self.mode_combo.addItem("Flat sample moved under the camera (scans)", "scans")
        self.mode_combo.addItem("Camera turned to look around (panorama)", "panorama")
        self.options.addRow("Mode", self.mode_combo)
        self.crop_check = QCheckBox("Crop off the black border")
        self.crop_check.setChecked(True)
        self.options.addRow(self.crop_check)

    def settings(self):
        return {"mode": self.mode_combo.currentData(), "crop": self.crop_check.isChecked()}

    @staticmethod
    def process(images, settings):
        return stitch(images, mode=settings["mode"], crop=settings["crop"])


class FluorescenceDialog(ImageSetDialog):
    title = "Fluorescence composite"
    short_name = "composite"
    help_text = ("Colors each channel image (one per filter/stain) and adds them together. "
                 "Colors are guessed from file names (DAPI, FITC, GFP, TRITC, ...) where possible. "
                 "Auto levels stretches each channel from its faintest to its brightest signal.")
    min_images = 1
    extra_columns = ["Color", "Auto levels", "Brightness"]
    DEFAULT_ORDER = ["Blue", "Green", "Red", "Magenta", "Cyan", "Yellow"]

    def row_widgets(self, path):
        color = QComboBox()
        color.addItems(list(COLORS))
        guess = guess_color(Path(path).name)
        if guess is None:  # blue, green, red, ... by position
            guess = self.DEFAULT_ORDER[(self.table.rowCount() - 1) % len(self.DEFAULT_ORDER)]
        color.setCurrentText(guess)
        auto = QCheckBox()
        auto.setChecked(True)
        auto.setStyleSheet("margin-left: 12px;")
        brightness = QDoubleSpinBox()
        brightness.setRange(0.1, 10.0)
        brightness.setSingleStep(0.1)
        brightness.setValue(1.0)
        brightness.setToolTip("Multiplies the channel after levels. Above 1 brightens faint stains.")
        return [color, auto, brightness]

    def settings(self):
        channels = []
        for r in range(self.table.rowCount()):
            channels.append({
                "color": self.table.cellWidget(r, 1).currentText(),
                "auto_levels": self.table.cellWidget(r, 2).isChecked(),
                "brightness": self.table.cellWidget(r, 3).value(),
            })
        return {"channels": channels}

    @staticmethod
    def process(images, settings):
        channels = [Channel(im, s["color"], s["auto_levels"], brightness=s["brightness"])
                    for im, s in zip(images, settings["channels"])]
        return composite(channels)
