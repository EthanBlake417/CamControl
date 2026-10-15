"""Captures panel: thumbnails of the most recent images and videos in the
capture folder, or in another folder chosen in the panel.

- Shows the newest N images (N chosen by the user), newest first.
- "Same as output folder" (on by default) follows the Output box folder.
  Choosing a folder with "..." turns it off; turning it back on returns
  to the output folder.
- Updates by itself when files in the folder change (new captures, files
  copied in or deleted), using QFileSystemWatcher.
- Double-click a thumbnail to open it in the main view (videos play there).
  The image open in the main view is outlined; selected ones are tinted.
  Then the arrow keys (also Home/End/Page Up/Down) step to the next or
  previous one and open it; with Shift or Ctrl held they only select.
- Select several (Ctrl/Shift-click) and right-click to focus stack, HDR,
  stitch or combine them (see PROCESS_TOOLS).
- Delete (right-click or the Delete key) moves the selected images, and
  their .json settings files, to the Recycle Bin.
- Thumbnails are read and shrunk on background threads, so a big folder (or
  a slow network drive) doesn't freeze the window. They're cached until the
  file changes.
"""

import os
import subprocess
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime
from pathlib import Path

import cv2
from PySide6.QtCore import QFile, QFileSystemWatcher, QObject, QPointF, QRect, QRectF, QSize, Qt, QTimer, Signal
from PySide6.QtGui import (
    QAction,
    QColor,
    QFont,
    QFontMetrics,
    QIcon,
    QKeySequence,
    QPainter,
    QPalette,
    QPen,
    QPixmap,
    QPolygonF,
)
from PySide6.QtWidgets import (
    QAbstractItemView,
    QCheckBox,
    QFileDialog,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QListView,
    QListWidget,
    QListWidgetItem,
    QMenu,
    QMessageBox,
    QPushButton,
    QSpinBox,
    QStyle,
    QStyledItemDelegate,
    QStyleOptionViewItem,
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


def thumbnail_image(path: Path):
    """The file's image (a video's first frame) shrunk to fit THUMB_SIZE, as
    numpy; None if unreadable. Safe to call off the GUI thread."""
    img = load_video_frame(path) if is_video(path) else load_image_file(path)
    if img is None:
        return None
    h, w = img.shape[:2]
    scale = min(THUMB_SIZE.width() / w, THUMB_SIZE.height() / h)
    return cv2.resize(img, (max(1, round(w * scale)), max(1, round(h * scale))), interpolation=cv2.INTER_AREA)


def thumbnail_icon(path: Path, small) -> QIcon:
    """thumbnail_image's result as an icon, videos marked. GUI thread only."""
    pixmap = QPixmap.fromImage(to_qimage(small))
    if is_video(path):
        draw_play_mark(pixmap)
    return QIcon(pixmap)


class _ThumbnailLoader(QObject):
    """Reads thumbnails on background threads; loaded is emitted on the GUI thread."""
    loaded = Signal(str, float, object)  # path, mtime, small image (None: unreadable)

    def __init__(self, parent=None):
        super().__init__(parent)
        self._pool = ThreadPoolExecutor(2, thread_name_prefix="thumbnails")
        self._generation = 0  # bumped by cancel(): older work is skipped

    def load(self, path: str, mtime: float):
        generation = self._generation

        def work():
            if generation == self._generation:  # not cancelled while waiting
                self.loaded.emit(path, mtime, thumbnail_image(Path(path)))
        self._pool.submit(work)

    def cancel(self):
        """Skip everything not started yet (e.g. another folder is shown)."""
        self._generation += 1


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


CELL_PAD = 6  # px of space round the thumbnail and name, the same on every side


class _ThumbnailDelegate(QStyledItemDelegate):
    """Lays out each cell itself, so the thumbnail sits centred with even
    space all round and the name on one line under it (shortened in the
    middle if too long; the tooltip has it in full). One color (the
    system's highlight color), two marks: selected thumbnails get a light
    tint (also while the list isn't focused), and the image open in the
    main view a solid outline and a bold name."""

    def __init__(self, panel: "GalleryPanel"):
        super().__init__(panel)
        self._panel = panel

    def sizeHint(self, option: QStyleOptionViewItem, index) -> QSize:
        return QSize(THUMB_SIZE.width() + 2 * CELL_PAD,
                     THUMB_SIZE.height() + option.fontMetrics.height() + 3 * CELL_PAD)

    def paint(self, painter: QPainter, option: QStyleOptionViewItem, index):
        selected = bool(option.state & QStyle.StateFlag.State_Selected)
        is_open = self._panel.is_open(index.data(PATH_ROLE))
        accent = option.palette.color(QPalette.ColorGroup.Active, QPalette.ColorRole.Highlight)
        cell = QRectF(option.rect).adjusted(1, 1, -1, -1)
        painter.save()
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        if selected:
            tint = QColor(accent)
            tint.setAlpha(55)
            painter.setPen(Qt.PenStyle.NoPen)
            painter.setBrush(tint)
            painter.drawRoundedRect(cell, 5, 5)

        # Thumbnail: centred in the THUMB_SIZE area at the top of the cell.
        r = option.rect
        area = QRect(r.x() + (r.width() - THUMB_SIZE.width()) // 2, r.y() + CELL_PAD, THUMB_SIZE.width(),
                     THUMB_SIZE.height())
        icon = index.data(Qt.ItemDataRole.DecorationRole)
        if isinstance(icon, QIcon):
            pixmap = icon.pixmap(THUMB_SIZE)
            size = pixmap.deviceIndependentSize().toSize()
            painter.drawPixmap(QRect(area.x() + (area.width() - size.width()) // 2,
                                     area.y() + (area.height() - size.height()) // 2, size.width(), size.height()),
                               pixmap)

        # Name: one line, centred under it.
        font = QFont(option.font)
        font.setBold(is_open)
        painter.setFont(font)
        painter.setPen(option.palette.color(QPalette.ColorRole.Text))
        text_rect = QRect(r.x() + CELL_PAD, area.bottom() + 1 + CELL_PAD, r.width() - 2 * CELL_PAD,
                          option.fontMetrics.height())
        name = QFontMetrics(font).elidedText(index.data(Qt.ItemDataRole.DisplayRole) or "",
                                             Qt.TextElideMode.ElideMiddle, text_rect.width())
        painter.drawText(text_rect, Qt.AlignmentFlag.AlignCenter, name)

        if is_open:
            painter.setBrush(Qt.BrushStyle.NoBrush)
            painter.setPen(QPen(accent, 2))
            painter.drawRoundedRect(cell.adjusted(0.5, 0.5, -0.5, -0.5), 5, 5)
        painter.restore()


class _ThumbnailList(QListWidget):
    """The thumbnails; tells when the keyboard moved to another one."""
    stepped = Signal(QListWidgetItem)  # moved to this item with a key, no Shift/Ctrl

    def keyPressEvent(self, event):
        before = self.currentItem()
        super().keyPressEvent(event)
        selecting = event.modifiers() & (Qt.KeyboardModifier.ShiftModifier | Qt.KeyboardModifier.ControlModifier)
        current = self.currentItem()
        if current is not None and current is not before and not selecting:
            self.stepped.emit(current)


class GalleryPanel(QWidget):
    open_requested = Signal(str)  # image path
    folder_changed = Signal()     # files in the folder were added, removed or changed
    process_requested = Signal(str, list)  # tool key (see PROCESS_TOOLS), image paths
    compare_requested = Signal(list)       # add these images to the compare grid

    def __init__(self, parent=None):
        super().__init__(parent)
        self.folder: Path | None = None         # the folder shown
        self.output_folder: Path | None = None  # the Output box folder, shown when following it
        self._cache: dict[tuple[str, float], QIcon] = {}  # (path, mtime) -> thumbnail
        self._shown: list[tuple[str, float]] = []         # what the list shows now
        self._open: str | None = None                     # normcase path of the image open in the main view

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

        folder_row = QHBoxLayout()
        folder_row.addWidget(QLabel("Folder"))
        self.folder_edit = QLineEdit()
        self.folder_edit.setReadOnly(True)
        folder_row.addWidget(self.folder_edit, 1)
        browse = QPushButton("...")
        browse.setFixedWidth(30)
        browse.setToolTip("Choose the folder to show")
        browse.clicked.connect(self._choose_folder)
        folder_row.addWidget(browse)
        layout.addLayout(folder_row)
        self.follow_check = QCheckBox("Same as output folder")
        self.follow_check.setChecked(True)
        self.follow_check.setToolTip("Show the folder captures are saved to")
        self.follow_check.toggled.connect(self._on_follow_toggled)
        layout.addWidget(self.follow_check)

        self.list = _ThumbnailList()
        self.list.setViewMode(QListView.ViewMode.IconMode)
        self.list.setIconSize(THUMB_SIZE)
        self.list.setResizeMode(QListView.ResizeMode.Adjust)
        self.list.setMovement(QListView.Movement.Static)
        self.list.setUniformItemSizes(True)
        self.list.setSpacing(4)
        self.list.setSelectionMode(QAbstractItemView.SelectionMode.ExtendedSelection)
        self.list.setItemDelegate(_ThumbnailDelegate(self))
        self.list.itemDoubleClicked.connect(lambda item: self.open_requested.emit(item.data(PATH_ROLE)))
        self.list.stepped.connect(lambda item: self.open_requested.emit(item.data(PATH_ROLE)))
        self.list.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self.list.customContextMenuRequested.connect(self._context_menu)
        # Delete key, only while the thumbnail list has focus.
        self.delete_action = QAction("Delete", self.list)
        self.delete_action.setShortcut(QKeySequence.StandardKey.Delete)
        self.delete_action.setShortcutContext(Qt.ShortcutContext.WidgetShortcut)
        self.delete_action.triggered.connect(self.delete_selected)
        self.list.addAction(self.delete_action)
        layout.addWidget(self.list)

        hint = QLabel("Double-click to open (outlined), then the arrow keys step through. Ctrl/Shift-click to "
                      "select several, right-click for tools. Delete moves to the Recycle Bin.")
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

        self._loader = _ThumbnailLoader(self)
        self._loader.loaded.connect(self._on_thumbnail)

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

    def set_open_path(self, path: str | None):
        """The image now shown in the main view (None: live, or not a file): outlined."""
        self._open = os.path.normcase(os.path.abspath(path)) if path else None
        self.list.viewport().update()

    def is_open(self, path: str | None) -> bool:
        return path is not None and self._open is not None and os.path.normcase(os.path.abspath(path)) == self._open

    @property
    def follows_output(self) -> bool:
        return self.follow_check.isChecked()

    def set_output_folder(self, folder):
        """The Output box folder changed. Shown if "Same as output folder" is on."""
        self.output_folder = Path(folder)
        if self.follows_output:
            self.set_folder(folder)

    def set_follow_output(self, follow: bool, folder=None):
        """Restore the panel's state: follow the output folder, or show `folder`."""
        if not follow and folder:
            self.follow_check.blockSignals(True)
            self.follow_check.setChecked(False)
            self.follow_check.blockSignals(False)
            self.set_folder(folder)
        else:
            self.follow_check.setChecked(True)

    def set_folder(self, folder):
        self.folder = Path(folder)
        self.folder_edit.setText(str(self.folder))
        self.folder_edit.setToolTip(str(self.folder))
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
        self._loader.cancel()
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
                self._loader.load(path, mtime)
        if select:
            self._select(select)

    # --- internals -----------------------------------------------------------------

    def _choose_folder(self):
        start = str(self.folder) if self.folder else ""
        folder = QFileDialog.getExistingDirectory(self, "Folder to show", start)
        if not folder:
            return
        self.follow_check.blockSignals(True)
        self.follow_check.setChecked(self.output_folder is not None
                                     and os.path.normcase(Path(folder)) == os.path.normcase(self.output_folder))
        self.follow_check.blockSignals(False)
        self.set_folder(folder)

    def _on_follow_toggled(self, follow: bool):
        if follow and self.output_folder is not None:
            self.set_folder(self.output_folder)

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

    def _on_thumbnail(self, path: str, mtime: float, small):
        if small is None:
            return  # unreadable (maybe still being written); placeholder stays
        icon = thumbnail_icon(Path(path), small)
        self._cache[(path, mtime)] = icon
        if (path, mtime) not in self._shown:
            return  # no longer shown (kept for when it is again)
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
        n = len(images)
        compare = menu.addAction(f"Add to compare ({n} image{'s' if n != 1 else ''})",
                                 lambda: self.compare_requested.emit(images))
        compare.setEnabled(n > 0)
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
