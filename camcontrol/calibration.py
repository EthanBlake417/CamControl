"""Calibration: how big one image pixel is, separately in X and Y.

Minimal for now: measurements use PIXELS (1 px = 1 px) until Phase 2 adds
measuring a stage micrometer and saving named calibrations to
calibrations/calibrations.json.

X and Y are kept separate because the camera's pixels aren't confirmed to
be square. Calibrations only apply to images of the size they were made on:
an HD2-style 3264x1836 capture has 1920/3264 the µm/px of a 1080p frame.
"""

from dataclasses import dataclass


@dataclass(frozen=True)
class Calibration:
    name: str = "pixels"
    x: float = 1.0      # units per pixel, horizontally
    y: float = 1.0      # units per pixel, vertically
    unit: str = "px"    # e.g. "µm"


PIXELS = Calibration()
