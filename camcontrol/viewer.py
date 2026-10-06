"""Live view window for the microscope camera.

Run with:
    python -m camcontrol.viewer

Keys:
    q or Esc   quit
    f          toggle the on-screen info
    e / d      exposure longer / shorter (each step doubles or halves it)
    g / b      gain up / down
    Space      capture a still to captures/ (image + .json settings)
    a          cycle frames averaged per capture (1, 4, 8, 16, 32)
    r          toggle save size: native / HD2-style 1.7x upscale
    x          toggle grid
    c          toggle crosshair (marks the centre of the camera frame)
    + / -      zoom in / out (display only; captures are never zoomed)
    0          reset zoom
    arrows     pan while zoomed

The window can be resized by dragging; the image scales to fit.
Exposure and gain are stored in the camera, so they stay set after quitting.
"""

import time

import cv2

from camcontrol.camera import Camera, exposure_seconds, format_exposure
from camcontrol.capture import HD2_SCALE, grab_average, save_capture, scaled_size
from camcontrol.overlays import View, draw_crosshair, draw_grid

WINDOW = "CamControl"
GAIN_STEP = 4
AVERAGE_CHOICES = [1, 4, 8, 16, 32]
MESSAGE_SECONDS = 3

# cv2.waitKeyEx codes for the arrow keys on Windows.
KEY_LEFT, KEY_UP, KEY_RIGHT, KEY_DOWN = 2424832, 2490368, 2555904, 2621440
ARROWS = {KEY_LEFT: (-1, 0), KEY_RIGHT: (1, 0), KEY_UP: (0, -1), KEY_DOWN: (0, 1)}


def draw_info(frame, lines: list[str]):
    """Write lines of text in the top-left corner (on the frame, in place)."""
    y = 50
    for text in lines:
        # Black outline under white text so it's readable on any background.
        for color, thickness in (((0, 0, 0), 6), ((255, 255, 255), 2)):
            cv2.putText(frame, text, (20, y), cv2.FONT_HERSHEY_SIMPLEX, 1.2,
                        color, thickness, cv2.LINE_AA)
        y += 50


def main():
    cam = Camera()

    # WINDOW_NORMAL lets the user resize it; 1080p is bigger than many screens.
    cv2.namedWindow(WINDOW, cv2.WINDOW_NORMAL | cv2.WINDOW_KEEPRATIO)
    cv2.resizeWindow(WINDOW, 1280, 720)

    show_info = True
    show_grid = False
    show_crosshair = False
    view = None  # created once we know the frame size
    fps = 0.0
    last = time.perf_counter()
    average_idx = 0
    scale = 1.0
    message, message_until = "", 0.0

    while True:
        frame = cam.read()

        if frame is not None:
            # Smoothed frame rate so the number doesn't flicker.
            now = time.perf_counter()
            fps = 0.9 * fps + 0.1 * (1.0 / max(now - last, 1e-6))
            last = now

            h, w = frame.shape[:2]
            if view is None or (view.frame_w, view.frame_h) != (w, h):
                view = View(w, h)

            # Everything below draws on the display copy, not the camera frame.
            shown = view.apply(frame)
            if show_grid:
                draw_grid(shown)
            if show_crosshair:
                draw_crosshair(shown, view)

            if show_info:
                exp, gain = cam.exposure, cam.gain
                n_avg = AVERAGE_CHOICES[average_idx]
                save_w, save_h = scaled_size(w, h, scale)
                size_note = " (upscaled)" if scale != 1 else ""
                lines = [
                    f"{w}x{h}  {fps:4.1f} fps",
                    (f"exposure {exp:g} (~{format_exposure(exp)})" if exp is not None else "exposure -")
                    + (f"   gain {gain:g}" if gain is not None else ""),
                    f"capture: avg {n_avg}  save {save_w}x{save_h}{size_note}",
                ]
                if view.zoom != 1:
                    lines.append(f"zoom {view.zoom:g}x")
                if time.perf_counter() < message_until:
                    lines.append(message)
                draw_info(shown, lines)
            cv2.imshow(WINDOW, shown)

        # waitKeyEx (not waitKey) so the arrow keys come through.
        full_key = cv2.waitKeyEx(1)
        key = full_key & 0xFF if full_key != -1 else -1
        if full_key in ARROWS:
            if view is not None:
                view.pan(*ARROWS[full_key])
        elif key in (ord("q"), 27):  # 27 = Esc
            break
        elif key == ord("f"):
            show_info = not show_info
        elif key == ord("e") and cam.exposure is not None:
            cam.set_exposure(cam.exposure + 1)
        elif key == ord("d") and cam.exposure is not None:
            cam.set_exposure(cam.exposure - 1)
        elif key == ord("g") and cam.gain is not None:
            cam.set_gain(cam.gain + GAIN_STEP)
        elif key == ord("b") and cam.gain is not None:
            cam.set_gain(cam.gain - GAIN_STEP)
        elif key == ord("a"):
            average_idx = (average_idx + 1) % len(AVERAGE_CHOICES)
        elif key == ord("r"):
            scale = HD2_SCALE if scale == 1 else 1.0
        elif key == ord("x"):
            show_grid = not show_grid
        elif key == ord("c"):
            show_crosshair = not show_crosshair
        elif key in (ord("+"), ord("=")) and view is not None:
            view.zoom_in()
        elif key == ord("-") and view is not None:
            view.zoom_out()
        elif key == ord("0") and view is not None:
            view.reset()
        elif key == ord(" "):
            n_avg = AVERAGE_CHOICES[average_idx]
            est = n_avg * max(exposure_seconds(cam.exposure) if cam.exposure is not None else 0, 1 / 30)
            print(f"Capturing {n_avg} frame(s), about {est:.1f} s...")
            image, got = grab_average(cam, n_avg)
            path = save_capture(
                image,
                exposure=cam.exposure,
                gain=cam.gain,
                frames_averaged=got,
                scale=scale,
            )
            print(f"Saved {path}")
            message, message_until = f"saved {path.name}", time.perf_counter() + MESSAGE_SECONDS

        # Closing the window with the X button also quits.
        if cv2.getWindowProperty(WINDOW, cv2.WND_PROP_VISIBLE) < 1:
            break

    cam.close()
    cv2.destroyAllWindows()


if __name__ == "__main__":
    main()
