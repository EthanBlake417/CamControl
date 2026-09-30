"""Fluorescence composites: color each channel image and add them together.

Each fluorescence filter gives one image of one stain (e.g. DAPI for nuclei,
FITC/GFP, TRITC/RFP). Only brightness matters in those images, so each one
is turned into brightness (the brightest color channel, so a green-looking
capture keeps its full signal), stretched between a black and a white level,
tinted with a chosen color, and the tinted images are added.

Run directly for a self-test:
    python -m camcontrol.processing.fluorescence
"""

from dataclasses import dataclass

import numpy as np

from camcontrol.processing.common import check_same_size

# Display name -> (R, G, B)
COLORS = {
    "Blue": (0, 0, 255),
    "Green": (0, 255, 0),
    "Red": (255, 0, 0),
    "Magenta": (255, 0, 255),
    "Cyan": (0, 255, 255),
    "Yellow": (255, 255, 0),
    "Gray": (255, 255, 255),
}

# Words in a file name that suggest the stain's usual display color.
NAME_HINTS = {
    "Blue": ("dapi", "hoechst", "blue", "uv"),
    "Green": ("fitc", "gfp", "green", "alexa488", "af488"),
    "Red": ("tritc", "rfp", "mcherry", "texas", "cy3", "red", "alexa594", "af594"),
    "Magenta": ("cy5", "far-red", "farred", "magenta"),
}


def guess_color(file_name: str) -> str | None:
    name = file_name.lower()
    for color, words in NAME_HINTS.items():
        if any(w in name for w in words):
            return color
    return None


@dataclass
class Channel:
    image: np.ndarray
    color: str = "Green"
    auto_levels: bool = True   # stretch from the 0.5th to the 99.9th percentile
    black: int = 0             # used when auto_levels is off
    white: int = 255
    brightness: float = 1.0


def intensity(image: np.ndarray) -> np.ndarray:
    return image.max(axis=2) if image.ndim == 3 else image


def composite(channels: list[Channel]) -> np.ndarray:
    """Tinted sum of the channels, as an 8-bit BGR image."""
    if not channels:
        raise ValueError("Add at least one channel.")
    check_same_size([c.image for c in channels])
    h, w = channels[0].image.shape[:2]
    out = np.zeros((h, w, 3), np.float32)
    for c in channels:
        signal = intensity(c.image).astype(np.float32)
        if c.auto_levels:
            black, white = np.percentile(signal, (0.5, 99.9))
        else:
            black, white = c.black, c.white
        norm = np.clip((signal - black) / max(white - black, 1.0), 0, 1) * c.brightness
        r, g, b = COLORS[c.color]
        out += norm[..., None] * np.array([b, g, r], np.float32)  # BGR order
    return np.clip(out + 0.5, 0, 255).astype(np.uint8)


if __name__ == "__main__":
    dapi = np.zeros((100, 100), np.uint8); dapi[20:40, 20:40] = 200
    gfp = np.zeros((100, 100), np.uint8); gfp[30:60, 30:60] = 90
    img = composite([Channel(dapi, "Blue"), Channel(gfp, "Green")])
    b, g, r = img[25, 25], img[50, 50], img[35, 35]
    print("only DAPI:", b, " only GFP:", g, " both:", r, " (B, G, R)")
    assert tuple(b) == (255, 0, 0) and tuple(g) == (0, 255, 0) and tuple(r) == (255, 255, 0)
    assert guess_color("sample_DAPI-001.tif") == "Blue"
    print("fluorescence OK")
