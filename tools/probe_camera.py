"""Phase 0: probe the microscope camera and report what works over USB.

What this does:
  1. Tries camera indices 0-5 on both the DirectShow and Media Foundation
     backends and reports which ones open and deliver a frame.
  2. For the chosen camera, tries a list of common resolutions and reports
     which ones actually take effect, plus the measured frame rate.
     One test frame per working resolution is saved to captures/probe/.
  3. Tries reading and changing exposure, gain, white balance, brightness,
     contrast (and a few others), and reports which ones respond. A control
     "responds" if the value reads back changed, or if the image itself
     visibly changes (mean brightness), since some drivers lie on read-back.
  4. Writes everything to captures/probe/report.json.

Usage:
    python tools/probe_camera.py                 # scan all, probe every camera found
    python tools/probe_camera.py --index 1       # probe only camera 1
    python tools/probe_camera.py --backend msmf  # use Media Foundation for the detail probe
    python tools/probe_camera.py --settings      # also open the driver's own settings dialog

Close HD2 (or any other camera app) first: Windows lets only one program
use the camera at a time.
"""

import argparse
import json
import os
import time
from datetime import datetime
from pathlib import Path

# Quiet OpenCV's console spam while scanning indices that don't exist.
# Must be set before importing cv2.
os.environ.setdefault("OPENCV_LOG_LEVEL", "ERROR")
# Media Foundation can take several seconds to open without this.
os.environ.setdefault("OPENCV_VIDEOIO_MSMF_ENABLE_HW_TRANSFORMS", "0")

import cv2  # noqa: E402
import numpy as np  # noqa: E402

BACKENDS = {
    "dshow": cv2.CAP_DSHOW,
    "msmf": cv2.CAP_MSMF,
}

MAX_INDEX = 5

RESOLUTIONS = [
    (640, 480),
    (1280, 720),
    (1920, 1080),
    (2592, 1944),
    (3264, 1836),
    (3840, 2160),
]

# Controls to test: name -> OpenCV property id.
CONTROLS = {
    "exposure": cv2.CAP_PROP_EXPOSURE,
    "auto_exposure": cv2.CAP_PROP_AUTO_EXPOSURE,
    "gain": cv2.CAP_PROP_GAIN,
    "brightness": cv2.CAP_PROP_BRIGHTNESS,
    "contrast": cv2.CAP_PROP_CONTRAST,
    "saturation": cv2.CAP_PROP_SATURATION,
    "sharpness": cv2.CAP_PROP_SHARPNESS,
    "gamma": cv2.CAP_PROP_GAMMA,
    "wb_temperature": cv2.CAP_PROP_WB_TEMPERATURE,
    "auto_wb": cv2.CAP_PROP_AUTO_WB,
}

# How far to nudge each control when testing it. DirectShow exposure is
# log2(seconds), e.g. -6 means 1/64 s, so a step of 2 is a 4x change.
NUDGE = {
    "exposure": 2,
    "wb_temperature": 1000,
    "gain": 10,
    "brightness": 20,
    "contrast": 20,
}
DEFAULT_NUDGE = 10

OUT_DIR = Path(__file__).resolve().parent.parent / "captures" / "probe"


def fourcc_to_str(value: float) -> str:
    """Turn OpenCV's numeric FOURCC (e.g. 1196444237.0) into text ('MJPG')."""
    code = int(value)
    if code <= 0:
        return "?"
    return "".join(chr((code >> (8 * i)) & 0xFF) for i in range(4))


def flush(cap: cv2.VideoCapture, n: int = 5):
    """Read and discard a few frames so a settings change has time to apply.

    Returns the last frame read (or None).
    """
    frame = None
    for _ in range(n):
        ok, f = cap.read()
        if ok:
            frame = f
    return frame


def mean_level(frame) -> float | None:
    """Average pixel value of a frame, used to see if a control changed the image."""
    if frame is None:
        return None
    return float(np.mean(frame))


# ---------------------------------------------------------------------------
# Step 1: which cameras exist?
# ---------------------------------------------------------------------------

def scan_indices() -> list[dict]:
    results = []
    for backend_name, backend_id in BACKENDS.items():
        for index in range(MAX_INDEX + 1):
            t0 = time.perf_counter()
            cap = cv2.VideoCapture(index, backend_id)
            opened = cap.isOpened()
            entry = {"backend": backend_name, "index": index, "opened": opened}
            if opened:
                ok, frame = cap.read()
                entry["frame_ok"] = bool(ok)
                if ok:
                    h, w = frame.shape[:2]
                    entry["default_size"] = [w, h]
                entry["fourcc"] = fourcc_to_str(cap.get(cv2.CAP_PROP_FOURCC))
                entry["open_seconds"] = round(time.perf_counter() - t0, 2)
            cap.release()
            results.append(entry)

            if opened:
                size = entry.get("default_size", "no frame")
                print(f"  [{backend_name:5}] index {index}: OPEN   default {size}")
            else:
                print(f"  [{backend_name:5}] index {index}: -")
    return results


# ---------------------------------------------------------------------------
# Step 2: resolutions and frame rate
# ---------------------------------------------------------------------------

def measure_fps(cap: cv2.VideoCapture, n_frames: int = 30) -> float | None:
    flush(cap, 5)  # let auto-exposure and buffers settle first
    t0 = time.perf_counter()
    got = 0
    for _ in range(n_frames):
        ok, _ = cap.read()
        if ok:
            got += 1
    elapsed = time.perf_counter() - t0
    if got == 0 or elapsed == 0:
        return None
    return round(got / elapsed, 1)


def probe_resolutions(index: int, backend_name: str, tag: str) -> list[dict]:
    """Try each resolution with a fresh camera handle, with and without MJPG.

    Many USB cameras only reach full frame rate at high resolutions when
    the compressed MJPG format is requested, so both are tested.
    """
    results = []
    for use_mjpg in (False, True):
        fmt = "MJPG" if use_mjpg else "default"
        for w, h in RESOLUTIONS:
            cap = cv2.VideoCapture(index, BACKENDS[backend_name])
            if not cap.isOpened():
                print(f"    could not reopen camera for {w}x{h}")
                continue
            # Size must be set before FOURCC, or DirectShow ignores the MJPG
            # request and falls back to slow uncompressed YUY2.
            cap.set(cv2.CAP_PROP_FRAME_WIDTH, w)
            cap.set(cv2.CAP_PROP_FRAME_HEIGHT, h)
            if use_mjpg:
                cap.set(cv2.CAP_PROP_FOURCC, cv2.VideoWriter_fourcc(*"MJPG"))

            frame = flush(cap, 3)
            entry = {"requested": [w, h], "format_requested": fmt}
            if frame is None:
                entry["frame_ok"] = False
                print(f"    {fmt:7} {w}x{h}: no frame")
            else:
                ah, aw = frame.shape[:2]
                entry.update(
                    frame_ok=True,
                    actual=[aw, ah],
                    took_effect=(aw, ah) == (w, h),
                    fourcc=fourcc_to_str(cap.get(cv2.CAP_PROP_FOURCC)),
                    reported_fps=round(cap.get(cv2.CAP_PROP_FPS), 1),
                    measured_fps=measure_fps(cap),
                )
                mark = "OK " if entry["took_effect"] else "-> "
                print(
                    f"    {fmt:7} {w}x{h}: {mark}{aw}x{ah}  "
                    f"fourcc={entry['fourcc']}  "
                    f"fps reported={entry['reported_fps']} measured={entry['measured_fps']}"
                )
                if entry["took_effect"]:
                    path = OUT_DIR / f"{tag}_{w}x{h}_{fmt}.png"
                    cv2.imwrite(str(path), frame)
                    entry["saved"] = path.name
            cap.release()
            results.append(entry)
    return results


# ---------------------------------------------------------------------------
# Step 3: controls
# ---------------------------------------------------------------------------

def probe_controls(cap: cv2.VideoCapture) -> dict:
    results = {}
    flush(cap, 10)  # let auto-exposure settle before testing anything

    for name, prop in CONTROLS.items():
        original = cap.get(prop)
        entry = {"initial": original}

        # A read-back of exactly -1 or 0 across the board usually means
        # "not supported", but we still try setting it to be sure.
        step = NUDGE.get(name, DEFAULT_NUDGE)
        target = original + step
        if name in ("auto_exposure", "auto_wb"):
            # Toggle-style controls: try flipping them.
            target = 0 if original != 0 else 1

        before = mean_level(flush(cap, 3))
        accepted = cap.set(prop, target)
        after_frame = flush(cap, 8)
        readback = cap.get(prop)
        after = mean_level(after_frame)

        entry.update(
            set_to=target,
            set_returned=bool(accepted),
            readback=readback,
            value_changed=readback != original,
            mean_before=None if before is None else round(before, 1),
            mean_after=None if after is None else round(after, 1),
        )
        # "Image changed" = average level moved by more than 2 (out of 255).
        if before is not None and after is not None:
            entry["image_changed"] = abs(after - before) > 2.0
        else:
            entry["image_changed"] = None

        entry["responds"] = bool(entry["value_changed"] or entry["image_changed"])

        # Put it back the way we found it.
        cap.set(prop, original)
        flush(cap, 5)

        results[name] = entry
        verdict = "RESPONDS" if entry["responds"] else "no effect"
        print(
            f"    {name:15} initial={original:<9g} set={target:<9g} "
            f"readback={readback:<9g} mean {entry['mean_before']}->{entry['mean_after']}  "
            f"{verdict}"
        )

    return results


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--index", type=int, help="camera index to probe in detail (default: every one found)")
    parser.add_argument("--backend", choices=BACKENDS, default="dshow", help="backend for the detail probe")
    parser.add_argument("--settings", action="store_true", help="open the driver settings dialog (DirectShow only)")
    args = parser.parse_args()

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    report = {
        "timestamp": datetime.now().isoformat(timespec="seconds"),
        "opencv_version": cv2.__version__,
    }

    print(f"OpenCV {cv2.__version__}")
    print("\n== Step 1: scanning camera indices ==")
    report["scan"] = scan_indices()

    if args.index is not None:
        indices = [args.index]
    else:
        indices = [
            e["index"] for e in report["scan"]
            if e["backend"] == args.backend and e.get("frame_ok")
        ]
    if not indices:
        print(f"\nNo working cameras on {args.backend}. Is HD2 still open?")
    report["cameras"] = {}

    for index in indices:
        tag = f"cam{index}_{args.backend}"
        print(f"\n== Camera {index} ({args.backend}) ==")

        print("  -- Step 2: resolutions --")
        resolutions = probe_resolutions(index, args.backend, tag)

        print("  -- Step 3: controls (at default resolution) --")
        cap = cv2.VideoCapture(index, BACKENDS[args.backend])
        controls = probe_controls(cap) if cap.isOpened() else {}

        if args.settings and args.backend == "dshow":
            print("  Opening driver settings dialog (close it to continue)...")
            cap.set(cv2.CAP_PROP_SETTINGS, 1)
            input("  Press Enter when done: ")
        cap.release()

        report["cameras"][str(index)] = {
            "backend": args.backend,
            "resolutions": resolutions,
            "controls": controls,
        }

    report_path = OUT_DIR / "report.json"
    report_path.write_text(json.dumps(report, indent=2))
    print(f"\nReport written to {report_path}")
    print(f"Test frames in {OUT_DIR}")


if __name__ == "__main__":
    main()
