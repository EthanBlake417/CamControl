"""Open the camera driver's own settings dialog, with a live preview.

This shows which controls (exposure, gain, etc.) the driver actually offers
and their ranges. Move a slider and watch the preview to see if it works.

Usage:
    python tools/camera_settings.py            # camera 0
    python tools/camera_settings.py --index 1

Press q or Esc in the preview window to quit.
"""

import argparse
import os

os.environ.setdefault("OPENCV_LOG_LEVEL", "ERROR")

import cv2  # noqa: E402


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--index", type=int, default=0)
    args = parser.parse_args()

    # The settings dialog is a DirectShow feature, so this uses DirectShow
    # (at the default 800x448, which runs at full frame rate).
    cap = cv2.VideoCapture(args.index, cv2.CAP_DSHOW)
    if not cap.isOpened():
        raise SystemExit(f"Could not open camera {args.index}. Close HD2 / OBS first.")

    cap.set(cv2.CAP_PROP_SETTINGS, 1)  # opens the driver's property dialog

    window = "Preview (q to quit)"
    cv2.namedWindow(window, cv2.WINDOW_NORMAL)
    while True:
        ok, frame = cap.read()
        if ok:
            cv2.imshow(window, frame)
        if cv2.waitKey(1) & 0xFF in (ord("q"), 27):
            break
        if cv2.getWindowProperty(window, cv2.WND_PROP_VISIBLE) < 1:
            break

    cap.release()
    cv2.destroyAllWindows()


if __name__ == "__main__":
    main()
