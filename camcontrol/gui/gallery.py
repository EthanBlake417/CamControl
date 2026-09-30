"""Captures panel: thumbnails of the most recent images and videos in the
capture folder.

- Shows the newest N images (N chosen by the user), newest first.
- Updates by itself when files in the folder change (new captures, files
  copied in or deleted), using QFileSystemWatcher.
- Double-click a thumbnail to open it in the main view (videos play there).
- Select several (Ctrl/Shift-click) and right-click to focus stack, HDR,
  stitch or combine them (see PROCESS_TOOLS).
- Delete (right-click or the Delete key) moves the selected images, and
  their .json settings files, to the Recycle Bin.
- Thumbnails load one at a time in the background of the event loop, so a
  big folder doesn't freeze the window. They're cached until the file changes.
"""

import os
import subprocess
from datetime import datetime
from pathlib import Path

import cv2
from PySide6.QtCore import QFile, QFileSystemWatcher, QPointF, QSize, Qt, QTimer, Signal
from PySide6.QtGui import QAction, QColor, QIcon, QKeySequence, QPainter, QPixmap, QPolygonF
from PySide6.QtWidgets import (
    QAbstractItemView,
    QHBoxLayout,
    QLabel,
    QListView,
    QListWidget,
    QListWidgetItem,
    QMenu,
    QMessageBox,
    QPushButton,
    QSpinBox,
    QVBoxLayout,
    QWidget,
)

from camcontrol.gui.qt_image import to_qimage
from camcontrol.image_io import IMAGE_EXTENSIONS, VIDEO_EXTENSIONS, is_video, load_image_file, load_video_frame

THUMB_SIZE = QSize(160, 90)
PATH_ROLE = Qt.ItemDataRole.UserRole
# Tools offered in the right-click menu when several images are selected:
# (key sent in process_requested, menu text).
PROCESS_TOOLS = [
    ("stack", "Focus stack..."),
    ("hdr", "HDR (exposure fusion)..."),
    ("stitch", "Stitch..."),
    ("composite", "Fluorescence composite..."),
]


def make_thumbnail(path: Path) -> QIcon | None:
    video = is_video(path)
    img = load_video_frame(path) if video else load_image_file(path)
    if img is None:
        return None
    h, w = img.shape[:2]
    scale = min(THUMB_SIZE.width() / w, THUMB_SIZE.height() / h)
    small = cv2.resize(img, (max(1, round(w * scale)), max(1, round(h * scale))),
                       interpolation=cv2.INTER_AREA)
    pixmap = QPixmap.fromImage(to_qimage(small))
    if video:
        draw_play_mark(pixmap)
    return QIcon(pixmap)


def draw_play_mark(pixmap: QPixmap):
    """A white play triangle in a dark circle, so videos stand out from images."""
    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    c = QPointF(pixmap.width() / 2, pixmap.height() / 2)
    r = min(pixmap.width(), pixmap.height()) * 0.22
    painter.setPen(Qt.PenStyle.NoPen)
    painter.setBrush(QColor(0, 0, 0, 150))
    painter.drawEllipse(c, r, r)
    painter.setBrush(QColor(255, 255, 255))
    k = r * 0.5
    painter.drawPolygon(QPolygonF([c + QPointF(-k * 0.7, -k), c + QPointF(-k * 0.7, k), c + QPointF(k, 0)]))
    painter.end()


class GalleryPanel(QWidget):
    open_requested = Signal(str)  # image path
    folder_changed = Signal()     # files in the folder were added, removed or changed
    process_requested = Signal(str, list)  # tool key (see PROCESS_TOOLS), image paths
    compare_requested = Signal(str)        # show this image next to the main view

    def __init__(self, parent=None):
        super().__init__(parent)
        self.folder: Path | None = None
        self._cache: dict[tuple[str, float], QIcon] = {}  # (path, mtime) -> thumbnail
        self._queue: list[str] = []                       # paths still to load
        self._shown: list[tuple[str, float]] = []         # what the list shows now

        layout = QVBoxLayout(self)
        top = QHBoxLayout()
        top.addWidget(QLabel("Show last"))
        self.count_spin = QSpinBox()
        self.count_spin.setRange(1, 500)
        self.count_spin.setValue(12)
        self.count_spin.valueChanged.connect(lambda _: self.refresh(force=True))
        top.addWidget(self.count_spin)
        top.addStretch()
        refresh = QPushButton("Refresh")
        refresh.clicked.connect(lambda: self.refresh(force=True))
        top.addWidget(refresh)
        layout.addLayout(top)

        self.list = QListWidget()
        self.list.setViewMode(QListView.ViewMode.IconMode)
        self.list.setIconSize(THUMB_SIZE)
        self.list.setResizeMode(QListView.ResizeMode.Adjust)
        self.list.setMovement(QListView.Movement.Static)
        self.list.setUniformItemSizes(True)
        self.list.setWordWrap(True)
        self.list.setSpacing(4)
        self.list.setSelectionMode(QAbstractItemView.SelectionMode.ExtendedSelection)
        self.list.itemDoubleClicked.connect(lambda item: self.open_requested.emit(item.data(PATH_ROLE)))
        self.list.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self.list.customContextMenuRequested.connect(self._context_menu)
        # Delete key, only while the thumbnail list has focus.
        self.delete_action = QAction("Delete", self.list)
        self.delete_action.setShortcut(QKeySequence.StandardKey.Delete)
        self.delete_action.setShortcutContext(Qt.ShortcutContext.WidgetShortcut)
        self.delete_action.triggered.connect(self.delete_selected)
        self.list.addAction(self.delete_action)
        layout.addWidget(self.list)

        hint = QLabel("Double-click to open. Ctrl/Shift-click to select several, "
                      "right-click for tools. Delete moves to the Recycle Bin.")
        hint.setWordWrap(True)
        hint.setStyleSheet("color: gray;")
        layout.addWidget(hint)

        placeholder = QPixmap(THUMB_SIZE)
        placeholder.fill(QColor(60, 60, 60))
        self._placeholder = QIcon(placeholder)

        # Refresh shortly after the folder changes. A capture writes two files
        # (image + .json), so wait a moment and refresh once.
        self._watcher = QFileSystemWatcher(self)
        self._watcher.directoryChanged.connect(lambda _: self._debounce.start())
        self._debounce = QTimer(self)
        self._debounce.setSingleShot(True)
        self._debounce.setInterval(300)
        self._debounce.timeout.connect(self._on_folder_changed)

        self._loader = QTimer(self)
        self._loader.setInterval(0)  # one thumbnail per trip through the event loop
        self._loader.timeout.connect(self._load_next)

    def sizeHint(self) -> QSize:
        # Default size when the panel first appears: two thumbnails wide.
        return QSize(380, 320)

    # --- public ----------------------------------------------------------------

    @property
    def count(self) -> int:
        return self.count_spin.value()

    def set_count(self, n: int):
        self.count_spin.setValue(n)

    def selected_paths(self) -> list[str]:
        """Selected images, oldest first (the order they were taken)."""
        rows = sorted((self.list.row(i), i.data(PATH_ROLE)) for i in self.list.selectedItems())
        return [path for _, path in reversed(rows)]  # the list shows newest first

    def set_folder(self, folder):
        self.folder = Path(folder)
        if self._watcher.directories():
            self._watcher.removePaths(self._watcher.directories())
        if self.folder.is_dir():
            self._watcher.addPath(str(self.folder))
        self.refresh(force=True)

    def refresh(self, force: bool = False, select: str | None = None):
        """Re-read the folder. Skips rebuilding if nothing changed."""
        if self.folder is not None and self.folder.is_dir() and not self._watcher.directories():
            self._watcher.addPath(str(self.folder))  # folder was created since set_folder
        files = self._recent_files()
        if files == self._shown and not force:
            if select:
                self._select(select)
            return
        if select is None:
            current = self.list.currentItem()
            select = current.data(PATH_ROLE) if current else None

        self._shown = files
        self.list.clear()
        self._queue = []
        for path, mtime in files:
            p = Path(path)
            item = QListWidgetItem(self._cache.get((path, mtime), self._placeholder), p.name)
            item.setData(PATH_ROLE, path)
            try:
                size = f"\n{p.stat().st_size / 1e6:.1f} MB"
            except OSError:  # deleted since the folder was scanned
                size = ""
            item.setToolTip(f"{p.name}\n{datetime.fromtimestamp(mtime):%Y-%m-%d %H:%M:%S}{size}")
            self.list.addItem(item)
            if (path, mtime) not in self._cache:
                self._queue.append(path)
        if select:
            self._select(select)
        if self._queue:
            self._loader.start()

    # --- internals -----------------------------------------------------------------

    def _on_folder_changed(self):
        self.refresh()
        self.folder_changed.emit()

    def _recent_files(self) -> list[tuple[str, float]]:
        if self.folder is None or not self.folder.is_dir():
            return []
        found = []
        for entry in os.scandir(self.folder):
            if entry.is_file() and Path(entry.name).suffix.lower() in IMAGE_EXTENSIONS | VIDEO_EXTENSIONS:
                found.append((entry.path, entry.stat().st_mtime))
        found.sort(key=lambda f: f[1], reverse=True)  # newest first
        return found[: self.count]

    def _select(self, path: str):
        for i in range(self.list.count()):
            item = self.list.item(i)
            if os.path.normcase(item.data(PATH_ROLE)) == os.path.normcase(path):
                self.list.setCurrentItem(item)
                self.list.scrollToItem(item)
                return

    def _load_next(self):
        if not self._queue:
            self._loader.stop()
            return
        path = self._queue.pop(0)
        mtimes = dict(self._shown)
        if path not in mtimes:
            return  # no longer shown
        icon = make_thumbnail(Path(path))
        if icon is None:
            return  # unreadable (maybe still being written); placeholder stays
        self._cache[(path, mtimes[path])] = icon
        for i in range(self.list.count()):
            item = self.list.item(i)
            if item.data(PATH_ROLE) == path:
                item.setIcon(icon)
                break

    def _context_menu(self, pos):
        item = self.list.itemAt(pos)
        if item is None:
            return
        path = item.data(PATH_ROLE)
        selected = self.selected_paths()
        images = [p for p in selected if not is_video(p)]  # the tools work on images only
        menu = QMenu(self)
        menu.addAction("Play" if is_video(path) else "Open", lambda: self.open_requested.emit(path))
        compare = menu.addAction("Compare side by side", lambda: self.compare_requested.emit(path))
        compare.setEnabled(not is_video(path))
        menu.addAction("Show in Explorer",
                       lambda: subprocess.Popen(["explorer", "/select,", os.path.normpath(path)]))
        menu.addSeparator()
        n = len(images)
        for key, text in PROCESS_TOOLS:
            action = menu.addAction(f"{text.rstrip('.')} ({n} image{'s' if n != 1 else ''})...",
                                    lambda k=key: self.process_requested.emit(k, images))
            # Fluorescence works on one image; the others need several.
            action.setEnabled(n >= (1 if key == "composite" else 2))
        menu.addSeparator()
        menu.addAction(f"Delete ({len(selected)} selected)" if len(selected) > 1 else "Delete",
                       self.delete_selected)
        menu.exec(self.list.viewport().mapToGlobal(pos))

    def delete_selected(self):
        """Move the selected images (and their .json files) to the Recycle Bin, after asking."""
        paths = self.selected_paths()
        if not paths:
            return
        what = Path(paths[0]).name if len(paths) == 1 else f"these {len(paths)} files"
        if QMessageBox.question(
            self, "Delete", f"Move {what} to the Recycle Bin?\n\nTheir .json settings files go too."
        ) != QMessageBox.StandardButton.Yes:
            return
        failed = []
        for path in paths:
            if not QFile.moveToTrash(path):
                failed.append(Path(path).name)
                continue
            sidecar = Path(path).with_suffix(".json")
            # Only if no other image shares the name (e.g. name-001.tif and name-001.png).
            if sidecar.is_file() and not any(
                    p.suffix.lower() in IMAGE_EXTENSIONS for p in sidecar.parent.glob(sidecar.stem + ".*")):
                QFile.moveToTrash(str(sidecar))
        self.refresh(force=True)
        self.folder_changed.emit()  # e.g. the next capture name
        if failed:
            QMessageBox.warning(self, "Delete", "Couldn't delete (maybe open in another program):\n"
                                + "\n".join(failed))
