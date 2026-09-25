"""Start the CamControl GUI.

    python -m camcontrol.app
    python -m camcontrol.app --index 1   # a different camera
"""

import argparse
import sys

from PySide6.QtWidgets import QApplication

from camcontrol.gui.main_window import MainWindow


def main():
    parser = argparse.ArgumentParser(description="CamControl GUI")
    parser.add_argument("--index", type=int, default=0, help="camera index (default 0)")
    args = parser.parse_args()

    app = QApplication(sys.argv)
    app.setApplicationName("CamControl")
    window = MainWindow(camera_index=args.index)
    window.show()
    sys.exit(app.exec())


if __name__ == "__main__":
    main()
