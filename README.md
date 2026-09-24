# CamControl

Python application for live viewing, capture, and calibrated measurement with a USB microscope camera. Built as a replacement for the parts of vendor imaging software (O.C. White "HD2 Advanced Imaging") used day to day.

## Status

Early development. Nothing works yet. Start with Phase 0 below.

## Hardware

- **Camera:** O.C. White 6MP Ultra-Cam II (SKU 14305), hybrid HDMI/USB
- **Optics:** O.C. White MacroZoom lens (or trinocular microscope)
- **Connection:** USB to a Windows PC. The camera appears as a camera device in Windows, so it should be readable with OpenCV and no vendor SDK.

Known camera facts (from the O.C. White MacroZoom spec sheet, 2021):

| Spec | Value |
|---|---|
| Live preview | 1920 x 1080 (2MP) |
| Still capture (onboard, to SD card) | 3264 x 1836 (6MP) |
| Video | 30 fps |
| Exposure range (vendor software) | 2 ms to 10 s |
| Sensor model / pixel size | **Not published** |

Unknowns to resolve early:

- What resolutions and frame rates the camera offers over USB (likely 1920 x 1080; maybe higher).
- Which controls work through the standard camera interface (exposure, gain, white balance, brightness, contrast). Focus is manual on the lens.
- Whether output pixels are square. The sensor is unconfirmed, so **calibrate X and Y separately** until measured.

## Goals

1. Live view from the camera with zoom, grid, and crosshair overlays.
2. Still capture to disk (PNG/TIFF) with metadata (timestamp, calibration used).
3. Calibrated measurement: lines, polylines, 3-point circles, angles, rectangles, polygons (area and perimeter).
4. Calibration table: µm-per-pixel for each zoom setting, separate X and Y values, saved to JSON.
5. Measurement export to CSV and Excel.
6. Image processing tools, usable on live captures or saved files (including full 6MP images from the SD card):
   - Flat-field correction
   - Extended depth of focus (focus stacking)
   - HDR / exposure fusion
   - Image stitching for large fields of view
   - Class counting (up to 5 categories, click to mark)
   - Fluorescence channel color mapping and composite

## Non-goals (for now)

- Replacing the camera's onboard HDMI/SD-card interface.
- RAW or 16-bit capture, long exposures, or other features that need the vendor's driver. No SDK is published.
- A polished GUI before the core tools work.

## Tech stack

- Python 3.14+
- `opencv-python`: camera capture, image processing, stitching, HDR
- `numpy`: array math
- `pandas` + `openpyxl`: CSV/Excel export
- `PySide6`: GUI (Qt for Python). Quick prototypes can use OpenCV windows first.

## Setup (Windows)

```powershell
py -3.14 -m venv .venv
.venv\Scripts\activate
pip install opencv-python numpy pandas openpyxl pyside6
```

Close HD2 before running anything. Windows lets only one program use the camera at a time.

## Project layout (planned)

```
camcontrol/
├── README.md
├── requirements.txt
├── camcontrol/
│   ├── camera.py        # open camera, set resolution/controls, grab frames
│   ├── viewer.py        # live view window, overlays, capture hotkeys
│   ├── calibration.py   # calibration table (JSON), per-axis µm/px
│   ├── measure.py       # measurement geometry and results
│   ├── export.py        # CSV / Excel output
│   └── processing/
│       ├── flatfield.py
│       ├── focus_stack.py
│       ├── hdr.py
│       ├── stitch.py
│       ├── count.py
│       └── fluorescence.py
├── tools/
│   └── probe_camera.py  # Phase 0 diagnostic
├── calibrations/
│   └── calibrations.json
└── captures/
```

## Roadmap

### Phase 0: Probe the camera
Write `tools/probe_camera.py` to:
- List camera indices 0–5 and which ones open, using both `cv2.CAP_DSHOW` and `cv2.CAP_MSMF` backends.
- For the microscope camera, try common resolutions (640x480, 1280x720, 1920x1080, 2592x1944, 3264x1836, 3840x2160) and report which ones actually take effect.
- Report the actual frame rate.
- Try reading and setting exposure, gain, white balance, brightness, and contrast, and report which respond.
- Save one test frame per working resolution.

Record the results in this README under **Hardware**.

### Phase 1: Live viewer and capture
- Live window at the best working resolution.
- Hotkeys: capture still, toggle grid, toggle crosshair, zoom in/out, quit.
- Save captures with a timestamped filename.

### Phase 2: Calibration
- Load an image of a stage micrometer, click two points a known distance apart, and store µm/px.
- Do it horizontally and vertically (rotate the micrometer 90°) and store X and Y separately.
- Report the X/Y ratio. If within ~1%, pixels can be treated as square.
- Save named entries (e.g. one per zoom setting) to `calibrations/calibrations.json`.

### Phase 3: Measurement
- Measurement tools on live or frozen frames, using the active calibration.
- Results list with export to CSV/Excel.
- Save an annotated copy of the image alongside the data.

### Phase 4: Processing tools
- Flat-field correction (divide by a blank reference frame).
- Focus stacking (align frames, pick sharpest pixels by Laplacian).
- HDR via `cv2.createMergeMertens`.
- Stitching via `cv2.Stitcher_create(cv2.Stitcher_SCANS)`.
- Class counting and fluorescence composites.

### Phase 5: GUI
- PySide6 app that combines the viewer, measurement panel, and processing tools.

## Useful open-source tools to compare against

- **Fiji (ImageJ)**: reference for measurement and stitching accuracy
- **focus-stack**: reference for focus-stacking quality
- **napari**: possible GUI base for later
- **Micro-Manager / pycromanager**: scripted acquisition, if automation is needed later

## Notes for Claude (AI assistant context)

- The user runs Windows and develops with Claude Desktop.
- No vendor SDK exists. All camera access goes through OpenCV's standard camera interface. Don't suggest vendor APIs.
- Keep modules small and independently runnable (`python -m camcontrol.viewer`), so each tool can be tested alone.
- Measurements must always use per-axis calibration (µm/px in X and Y), never a single scale, until squareness is confirmed.
- Processing tools should accept file paths as input, not only live frames, so full-resolution SD card images can be used.
- Target Python 3.14+ and use PySide6 (not PyQt) for all GUI code.
- Prefer clear, commented code over clever code. The user is building this to learn and to own it.

