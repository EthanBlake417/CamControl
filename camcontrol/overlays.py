"""Display-only overlays for the live view: digital zoom/pan, grid, crosshair.

These change what's shown on screen, never what's captured. Captures always
save the raw camera frame.

View geometry:
  The view is a crop of the full frame, centred at (cx, cy) in frame pixels,
  of size (frame_w / zoom, frame_h / zoom), scaled back up to the frame size.
  Anything drawn in "frame coordinates" (like the crosshair at the optical
  centre) is mapped into the view with View.to_view().
"""

from dataclasses import dataclass

import cv2

ZOOM_LEVELS = [1, 1.5, 2, 3, 4, 6, 8]
PAN_FRACTION = 0.1  # each pan step moves 10% of the visible width/height

GRID_COLOR = (0, 255, 255)       # yellow (BGR)
CROSSHAIR_COLOR = (0, 0, 255)    # red (BGR)


@dataclass
class View:
    frame_w: int
    frame_h: int
    zoom_idx: int = 0
    cx: float | None = None
    cy: float | None = None

    def __post_init__(self):
        if self.cx is None:
            self.reset()

    @property
    def zoom(self) -> float:
        return ZOOM_LEVELS[self.zoom_idx]

    def reset(self):
        self.zoom_idx = 0
        self.cx = self.frame_w / 2
        self.cy = self.frame_h / 2

    def zoom_in(self):
        self.zoom_idx = min(self.zoom_idx + 1, len(ZOOM_LEVELS) - 1)
        self._clamp()

    def zoom_out(self):
        self.zoom_idx = max(self.zoom_idx - 1, 0)
        self._clamp()

    def pan(self, dx_steps: int, dy_steps: int):
        """Move the view by a number of steps (negative = left/up)."""
        self.cx += dx_steps * PAN_FRACTION * self.frame_w / self.zoom
        self.cy += dy_steps * PAN_FRACTION * self.frame_h / self.zoom
        self._clamp()

    def _crop_size(self) -> tuple[float, float]:
        return self.frame_w / self.zoom, self.frame_h / self.zoom

    def _clamp(self):
        """Keep the crop inside the frame."""
        cw, ch = self._crop_size()
        self.cx = min(max(self.cx, cw / 2), self.frame_w - cw / 2)
        self.cy = min(max(self.cy, ch / 2), self.frame_h - ch / 2)

    def _origin(self) -> tuple[int, int]:
        """Top-left corner of the crop, in frame pixels."""
        cw, ch = self._crop_size()
        return int(round(self.cx - cw / 2)), int(round(self.cy - ch / 2))

    def apply(self, frame):
        """Return the zoomed/panned view of a frame (same size as the frame)."""
        if self.zoom == 1:
            return frame.copy()
        cw, ch = self._crop_size()
        x0, y0 = self._origin()
        crop = frame[y0:y0 + int(round(ch)), x0:x0 + int(round(cw))]
        # At high zoom, show real pixels as blocks instead of blurring them.
        interp = cv2.INTER_NEAREST if self.zoom >= 4 else cv2.INTER_LINEAR
        return cv2.resize(crop, (self.frame_w, self.frame_h), interpolation=interp)

    def to_view(self, x: float, y: float) -> tuple[int, int]:
        """Map a point in frame pixels to view pixels."""
        x0, y0 = self._origin()
        return int(round((x - x0) * self.zoom)), int(round((y - y0) * self.zoom))


def draw_grid(img, rows: int = 8, cols: int = 8, color=GRID_COLOR):
    """Evenly spaced grid across the view (screen-fixed, like HD2's grid)."""
    h, w = img.shape[:2]
    for i in range(1, cols):
        x = round(i * w / cols)
        cv2.line(img, (x, 0), (x, h - 1), color, 1, cv2.LINE_AA)
    for j in range(1, rows):
        y = round(j * h / rows)
        cv2.line(img, (0, y), (w - 1, y), color, 1, cv2.LINE_AA)


def draw_crosshair(img, view: View, color=CROSSHAIR_COLOR):
    """Full-length crosshair through the centre of the camera frame.

    It stays on the optical centre when zoomed or panned, so it may move
    off-centre on screen (or off screen entirely).
    """
    h, w = img.shape[:2]
    x, y = view.to_view(view.frame_w / 2, view.frame_h / 2)
    cv2.line(img, (x, 0), (x, h - 1), color, 2, cv2.LINE_AA)
    cv2.line(img, (0, y), (w - 1, y), color, 2, cv2.LINE_AA)
    cv2.circle(img, (x, y), 20, color, 2, cv2.LINE_AA)
