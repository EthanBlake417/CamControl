"""Start the CamControl GUI.

    python main.py
    python -m camcontrol.app
    python -m camcontrol.app --index 1   # a different camera
"""

import argparse
import ctypes
import sys

from PySide6.QtGui import QIcon
from PySide6.QtWidgets import QApplication

from camcontrol.gui.main_window import MainWindow
from camcontrol.paths import APP_ICON

APP_ID = "GET.CamControl"


def main():
    parser = argparse.ArgumentParser(description="CamControl GUI")
    parser.add_argument("--index", type=int, default=None,
                        help="camera index (default: the camera used last, else the first)")
    args = parser.parse_args()

    # Without this, Windows groups the app under python.exe and shows the
    # Python icon in the taskbar instead of ours.
    if sys.platform == "win32":
        ctypes.windll.shell32.SetCurrentProcessExplicitAppUserModelID(APP_ID)

    app = QApplication(sys.argv)
    app.setApplicationName("CamControl")
    app.setWindowIcon(QIcon(str(APP_ICON)))  # every window, and the taskbar
    window = MainWindow(camera_index=args.index)
    window.show()
    sys.exit(app.exec())


if __name__ == "__main__":
    main()
