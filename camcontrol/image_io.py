"""Reading and writing image files."""

import json
import struct
import zlib
from pathlib import Path

import cv2
import numpy as np

IMAGE_EXTENSIONS = {".tif", ".tiff", ".png", ".jpg", ".jpeg", ".bmp"}
VIDEO_EXTENSIONS = {".mp4", ".avi"}  # what the Recording panel writes


def is_video(path) -> bool:
    return Path(path).suffix.lower() in VIDEO_EXTENSIONS


def load_video_frame(path) -> np.ndarray | None:
    """The first frame of a video (for thumbnails), or None."""
    cap = cv2.VideoCapture(str(path))
    ok, frame = cap.read()
    cap.release()
    return frame if ok else None


def load_image_file(path) -> np.ndarray | None:
    """Read an image as 8-bit BGR or grayscale. Returns None if unreadable.

    Uses imdecode on the raw bytes, because cv2.imread can't open paths
    with non-ASCII characters on Windows.
    """
    try:
        data = np.fromfile(str(path), dtype=np.uint8)
    except OSError:
        return None
    img = cv2.imdecode(data, cv2.IMREAD_UNCHANGED)
    if img is None:
        return None
    if img.dtype == np.uint16:
        img = (img >> 8).astype(np.uint8)  # 16-bit -> 8-bit for display
    elif img.dtype != np.uint8:
        img = cv2.normalize(img, None, 0, 255, cv2.NORM_MINMAX).astype(np.uint8)
    if img.ndim == 3 and img.shape[2] == 4:
        img = cv2.cvtColor(img, cv2.COLOR_BGRA2BGR)
    return img


def save_image_file(path, image: np.ndarray, details: dict | None = None):
    """Write an image; the format comes from the file extension.

    details: the capture's settings, kept inside the image as JSON text (as the
    Rust app does): a TIFF's ImageDescription field, a PNG "Description" text
    chunk, or a JPEG comment (a BMP can't hold them). Image tools such as
    ImageJ/Fiji or exiftool show them. read_image_details reads them back.

    imencode + tofile instead of cv2.imwrite, which fails on Windows when
    the path has non-ASCII characters (e.g. "µ" in a name).
    """
    ext = (Path(path).suffix or ".tif").lower()
    params = [cv2.IMWRITE_JPEG_QUALITY, 95] if ext in (".jpg", ".jpeg") else []
    ok, data = cv2.imencode(ext, image, params)
    if not ok:
        raise RuntimeError(f"Could not encode image as {ext}")
    data = data.tobytes()
    if details is not None:
        text = json.dumps(details, separators=(",", ":")).encode("ascii")  # non-ASCII as \uXXXX
        if ext in (".tif", ".tiff"):
            data = _tiff_with_description(data, text)
        elif ext == ".png":
            data = _png_with_text(data, b"Description", text)
        elif ext in (".jpg", ".jpeg"):
            text = text[:65000]  # a segment holds at most 64 KB
            data = data[:2] + b"\xff\xfe" + struct.pack(">H", len(text) + 2) + text + data[2:]
    Path(path).write_bytes(data)


def _png_with_text(data: bytes, keyword: bytes, text: bytes) -> bytes:
    """data (a PNG) with an iTXt chunk (uncompressed UTF-8 text) right after the header chunk."""
    body = keyword + b"\0" + b"\0\0" + b"\0" + b"\0" + text  # no compression, no language, no translation
    chunk = struct.pack(">I", len(body)) + b"iTXt" + body + struct.pack(">I", zlib.crc32(b"iTXt" + body))
    end_of_header = 8 + 8 + struct.unpack(">I", data[8:12])[0] + 4  # signature, then IHDR
    return data[:end_of_header] + chunk + data[end_of_header:]


def _tiff_with_description(data: bytes, text: bytes) -> bytes:
    """data (a classic TIFF) with an ImageDescription field (tag 270) in its first
    image's directory: a copy of the directory with it added goes at the end of
    the file, and the header points to it. Everything else stays where it was."""
    order = "<" if data[:2] == b"II" else ">"
    ifd = struct.unpack(order + "I", data[4:8])[0]
    n = struct.unpack(order + "H", data[ifd:ifd + 2])[0]
    entries = [data[ifd + 2 + 12 * i: ifd + 14 + 12 * i] for i in range(n)]
    next_ifd = data[ifd + 2 + 12 * n: ifd + 6 + 12 * n]
    entries = [e for e in entries if struct.unpack(order + "H", e[:2])[0] != 270]
    out = bytearray(data)
    if len(out) % 2:
        out += b"\0"
    text_at = len(out)
    out += text + b"\0"
    if len(out) % 2:
        out += b"\0"
    entries.append(struct.pack(order + "HHII", 270, 2, len(text) + 1, text_at))  # 2: ASCII
    entries.sort(key=lambda e: struct.unpack(order + "H", e[:2])[0])
    new_ifd = len(out)
    out += struct.pack(order + "H", len(entries)) + b"".join(entries) + next_ifd
    out[4:8] = struct.pack(order + "I", new_ifd)
    return bytes(out)


def _tiff_description(data: bytes) -> bytes | None:
    if data[:2] not in (b"II", b"MM"):
        return None
    order = "<" if data[:2] == b"II" else ">"
    ifd = struct.unpack(order + "I", data[4:8])[0]
    n = struct.unpack(order + "H", data[ifd:ifd + 2])[0]
    for i in range(n):
        tag, kind, count, value = struct.unpack(order + "HHII", data[ifd + 2 + 12 * i: ifd + 14 + 12 * i])
        if tag == 270 and kind == 2:
            raw = data[ifd + 10 + 12 * i: ifd + 10 + 12 * i + count] if count <= 4 else data[value:value + count]
            return raw.rstrip(b"\0")
    return None


def _png_text(data: bytes, keyword: bytes) -> bytes | None:
    i = 8
    while i + 8 <= len(data):
        length, kind = struct.unpack(">I", data[i:i + 4])[0], data[i + 4:i + 8]
        body = data[i + 8:i + 8 + length]
        if kind in (b"iTXt", b"tEXt") and body.startswith(keyword + b"\0"):
            rest = body[len(keyword) + 1:]
            if kind == b"tEXt":
                return rest
            if rest[0] != 0:  # compressed
                return None
            rest = rest[2:]
            rest = rest[rest.index(b"\0") + 1:]  # language tag
            return rest[rest.index(b"\0") + 1:]  # translated keyword
        if kind == b"IDAT":
            break
        i += 12 + length
    return None


def _jpeg_comment(data: bytes) -> bytes | None:
    i = 2
    while i + 4 <= len(data) and data[i] == 0xFF:
        marker, length = data[i + 1], struct.unpack(">H", data[i + 2:i + 4])[0]
        if marker == 0xFE:
            return data[i + 4:i + 2 + length]
        if marker == 0xDA:  # image data: no more headers
            break
        i += 2 + length
    return None


def read_image_details(path) -> dict | None:
    """The capture details saved inside an image (see save_image_file); for
    images saved before that, the .json file beside it."""
    path = Path(path)
    ext = path.suffix.lower()
    try:
        data = path.read_bytes() if ext in (".tif", ".tiff", ".png", ".jpg", ".jpeg") else b""
        if ext in (".tif", ".tiff"):
            text = _tiff_description(data)
        elif ext == ".png":
            text = _png_text(data, b"Description")
        elif ext in (".jpg", ".jpeg"):
            text = _jpeg_comment(data)
        else:
            text = None
        if text:
            details = json.loads(text.decode("utf-8", "replace"))
            if isinstance(details, dict):
                return details
    except (OSError, ValueError, IndexError, struct.error):
        pass
    try:
        details = json.loads(path.with_suffix(".json").read_text())
        return details if isinstance(details, dict) else None
    except (OSError, ValueError):
        return None


def details_summary(details: dict) -> str:
    """The saved details in one line for the status bar, e.g.
    "2026-10-20 18:17:03 · exposure 15 ms · gain 30 · 4 frames averaged"."""
    parts = []
    if isinstance(details.get("timestamp"), str):
        parts.append(details["timestamp"].replace("T", " "))
    s = details.get("exposure_seconds_approx")
    if isinstance(s, (int, float)) and not isinstance(s, bool):
        parts.append(f"exposure {round(s, 2):g} s" if s >= 1 else f"exposure {round(s * 1000, 2):g} ms")
    g = details.get("gain")
    if isinstance(g, (int, float)) and not isinstance(g, bool):
        parts.append(f"gain {g:g}")
    for key, text in (("frames_averaged", "frames averaged"), ("frames_summed_per_exposure", "frames summed per exposure")):
        n = details.get(key)
        if isinstance(n, int) and not isinstance(n, bool) and n > 1:
            parts.append(f"{n} {text}")
    if details.get("upscaled") is True:
        parts.append("HD2 size")
    if isinstance(details.get("flat_field"), str):
        parts.append(f"flat-field {details['flat_field']}")
    if isinstance(details.get("video_frame"), int):
        parts.append(f"video frame {details['video_frame']}")
    if isinstance(details.get("source"), str) and details["source"]:
        parts.append(f"from {details['source']}")
    return "  ·  ".join(parts)
