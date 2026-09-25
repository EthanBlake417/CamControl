# CamControl

Python application for live viewing, capture, and calibrated measurement with a USB microscope camera. Built as a replacement for the parts of vendor imaging software (O.C. White "HD2 Advanced Imaging") used day to day.

## Status

Early development. Done: camera probe (Phase 0), live view and capture (Phase 1), measurement in pixels (Phase 3), and the GUI (Phase 5). Next: calibration (Phase 2), so measurements can be in µm, then the processing tools (Phase 4).

**GUI** (main app). Use PyCharm's Run button on `main.py`, or:

```powershell
venv\Scripts\python main.py
```

The GUI has a live image with zoom (mouse wheel) and pan (drag); sliders for exposure, gain, contrast, saturation and sharpness; capture with frame averaging, save size, format and folder; grid and crosshair overlays; freeze; opening image files; and a pixel readout (x, y, RGB) in the status bar. Settings are remembered between runs.

| Key | Action |
|---|---|
| Space | capture |
| `E` / `D` | exposure longer / shorter |
| `L` | live / freeze |
| `G` / `C` | grid / crosshair |
| `F` / `1` | fit to window / 100% |
| Ctrl+= / Ctrl+- | zoom in / out |
| Ctrl+O | open an image file |
| Ctrl+E | export measurements |
| Ctrl+1 / Ctrl+2 | show / hide the Controls / Measurements panel (also **View → Panels**, or the panel's × button) |

**Measuring.** Pick a tool in the Measurements panel (Line, Polyline, Circle (3 pt), Angle, Rectangle, Polygon) and click points on the image. Line, circle, angle and rectangle finish by themselves. For a polyline or polygon, double-click, right-click or press Enter to finish. Backspace removes the last point, Esc cancels (press it twice to go back to panning), and middle-drag pans while a tool is active. Results appear in the table, and selecting a row highlights that shape. **Export** writes the table (`.xlsx` with an Info sheet, or `.csv`), the measured image (`_image.png`) and an annotated copy (`_annotated.png`). Freeze the live view (L) or open a file before measuring, so the image doesn't change underneath you. Until calibration exists, all values are in pixels.

**Simple OpenCV viewer** (no Qt; handy for quick checks):

```powershell
venv\Scripts\python -m camcontrol.viewer
```

| Key | Action |
|---|---|
| `e` / `d` | exposure longer / shorter (x2 per step) |
| `g` / `b` | gain up / down |
| Space | capture a still (TIFF + `.json` settings) to `captures/` |
| `a` | frames averaged per capture: 1, 4, 8, 16, 32 |
| `r` | save size: native 1920x1080 / HD2-style 3264x1836 (upscaled) |
| `x` / `c` | grid / crosshair |
| `+` / `-` / `0` | zoom in / out / reset (display only) |
| arrows | pan while zoomed |
| `f` | toggle on-screen info |
| `q` / Esc | quit |

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

### Phase 0 findings (2026-09-24)

Measured with `tools/probe_camera.py`, `ffmpeg -list_options`, and timing tests.

**Device.** Windows sees it as **"MC802 USB2.0 Camera"** (USB VID `EBA1`, PID `7588`), camera index 0 on the dev PC. It uses Microsoft's generic UVC driver (`usbvideo.sys`) and exposes a single video interface, with no still-image pin and no vendor interface.

**Modes over USB** (the complete list the driver advertises):

| Size | MJPG | YUY2 (uncompressed) |
|---|---|---|
| 1920 x 1080 | 30 fps | 7 fps |
| 1280 x 720 | 30 fps | 15 fps |
| 1024 x 576 | 30 fps | 20 fps |
| 800 x 448 | 30 fps | 30 fps |

**1920 x 1080 is the largest size available over USB.** Real 6MP (3264 x 1836) images exist only on the camera's SD card.

**Frames.** Frames come through OpenCV with DirectShow (`cv2.CAP_DSHOW`). Set the frame size **before** the MJPG FOURCC. In the other order, DirectShow silently uses YUY2 at 5.7 fps. With the right order, 1080p runs at about 25 fps. Media Foundation gives 30 fps but can't set controls reliably.

**Controls** are set through `camcontrol/uvc_controls.py`, which calls the Windows camera interfaces (`IAMCameraControl`, `IAMVideoProcAmp`) directly via `comtypes`, not through OpenCV. OpenCV caps gain at 32, and it can't get exposure working again after HD2 has used the camera. Run `python -m camcontrol.uvc_controls` to list every control, its range and its current value.

| Control | Range | Works? | Notes |
|---|---|---|---|
| Exposure | -13 to -3 | Yes | **Real exposure ≈ 10 × 2^value s** (about 1.2 ms to 1.25 s), not the nominal 2^value. Measured from frame timing. |
| Gain | 0 to 63 | Yes | Default 48. |
| Contrast | 0 to 15 | Yes | |
| Saturation | 0 to 15 | Yes | |
| Sharpness | 0 to 7 | Yes | |
| Brightness | 0 to 15 | No | Listed by the camera, but writes are ignored. |
| Gamma | 0 to 65535 | No | Listed by the camera, but writes are ignored. |

- **HD2 leaves exposure stuck.** After HD2 has used the camera, it ignores exposure writes that carry the "manual" flag until one write with no flags goes through. `Camera.set()` does this every time, and `Camera()` re-applies exposure when it opens. The camera always reports exposure as "auto", even when manual values are clearly in effect, so that flag can't be trusted.
- The early probe results showing contrast etc. as not working were wrong. The probe changed each value by +20, which is outside the 0 to 15 range.
- All control values are stored **in the camera**. They persist after the program closes, and other apps see them too.
- At exposures around 1 s and longer, OpenCV's DirectShow reader times out and returns all-black frames. `Camera.read()` skips those.
- Focus is manual on the lens.

**Still unknown:** whether pixels are square. **Calibrate X and Y separately** until this is measured.

### How HD2 works (from its install files and config)

HD2 "Advanced Imaging & Measurement" is rebranded **Tucsen** software (`TUCam.dll`, `TSCam*` plugins). It lists this camera by name and drives it through the same standard UVC interface. It sends no vendor-specific commands, so it has no camera access that CamControl lacks.

- **Its 3264 x 1836 files are 1080p frames scaled up on save.** HD2 has separate preview and save resolutions, plus an interpolation setting (`RS_RESOLUTIONSAVE`, `RS_RESOLUTIONINTER` in `CameraCfg.ini`). The pixel statistics of a sample HD2 TIFF match a 1.7x upscale: noise is correlated between neighbouring pixels, which it isn't in a native frame.
- **Its long exposures (e.g. "10 s") and low-noise captures come from software frame averaging or integration** (`PROCESS_AVERAGE`, `PROCESS_INTEGRAL`, `PROCESS_TIMEINTEGRAL`).
- Its bundled Tucsen USB3 driver (`tuusb3.sys`) isn't used by this camera.
- Settings are stored in `%LOCALAPPDATA%\HD2 Advanced Imaging & Measurement\`.

**Consequence for measurement:** µm/px calibrated on HD2's 3264 x 1836 files is 1920/3264 (≈ 0.588) times the value for native 1080p frames. Each capture's `.json` records `saved_size` and `upscaled` for this reason.

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
- `opencv-python` (5.x): camera capture, image processing, stitching, HDR
- `numpy`: array math
- `pandas` + `openpyxl`: CSV/Excel export
- `PySide6`: GUI (Qt for Python). Quick prototypes can use OpenCV windows first.

## Setup (Windows)

The environment is managed with [uv](https://docs.astral.sh/uv/). Dependencies are in `pyproject.toml`. The virtual environment lives in `venv/`, not uv's default `.venv/`.

```powershell
$env:UV_PROJECT_ENVIRONMENT = "venv"
uv sync
```

Close HD2 before running anything. Windows lets only one program use the camera at a time.

## Project layout (planned)

```
camcontrol/
├── README.md
├── main.py              # starts the GUI
├── assets/            # get_logo.ico (window/taskbar icon), get_logo.png (About box)
├── requirements.txt
├── camcontrol/
│   ├── app.py           # starts the GUI (main.py calls this)
│   ├── paths.py         # asset file locations
│   ├── camera.py        # open camera, set resolution/controls, grab frames
│   ├── uvc_controls.py  # direct DirectShow camera controls (comtypes)
│   ├── capture.py       # frame averaging, save image + JSON metadata
│   ├── overlays.py      # display-only zoom/pan, grid, crosshair
│   ├── viewer.py        # simple OpenCV live view (no Qt)
│   ├── gui/
│   │   ├── main_window.py   # window, controls panel, menus, status bar
│   │   ├── image_view.py    # zoom/pan image view, grid, crosshair, measurement tools
│   │   ├── measure_panel.py # tool buttons, results table, delete/clear/export
│   │   ├── measure_draw.py  # draws measurements (on screen and for the annotated export)
│   │   ├── qt_image.py      # numpy -> QImage
│   │   └── camera_worker.py # camera thread (keeps the GUI responsive)
│   ├── calibration.py   # per-axis µm/px (only "pixels" so far; JSON table in Phase 2)
│   ├── measure.py       # measurement geometry and results (self-test: python -m camcontrol.measure)
│   ├── export.py        # CSV / Excel output
│   └── processing/
│       ├── flatfield.py
│       ├── focus_stack.py
│       ├── hdr.py
│       ├── stitch.py
│       ├── count.py
│       └── fluorescence.py
├── tools/
│   ├── probe_camera.py  # Phase 0 diagnostic
│   └── camera_settings.py  # opens the driver's own settings dialog
├── calibrations/
│   └── calibrations.json
└── captures/
```

## Roadmap

### Phase 0: Probe the camera (done)
`tools/probe_camera.py`:
- List camera indices 0–5 and which ones open, using both `cv2.CAP_DSHOW` and `cv2.CAP_MSMF` backends.
- For the microscope camera, try common resolutions (640x480, 1280x720, 1920x1080, 2592x1944, 3264x1836, 3840x2160) and report which ones actually take effect.
- Report the actual frame rate.
- Try reading and setting exposure, gain, white balance, brightness, and contrast, and report which respond.
- Save one test frame per working resolution.

Results are recorded above under **Phase 0 findings**.

### Phase 1: Live viewer and capture (done)
- Live window at 1920x1080 (DirectShow, MJPG).
- Hotkeys: exposure, gain, capture still, frame averaging, save size, grid, crosshair, zoom/pan, quit.
- Captures saved with a timestamped filename and a `.json` sidecar (exposure, gain, frames averaged, sizes).

### Phase 2: Calibration
- Load an image of a stage micrometer, click two points a known distance apart, and store µm/px.
- Do it horizontally and vertically (rotate the micrometer 90°) and store X and Y separately.
- Report the X/Y ratio. If within ~1%, pixels can be treated as square.
- Save named entries (e.g. one per zoom setting) to `calibrations/calibrations.json`.

### Phase 3: Measurement (done, in pixels)
- Done: line, polyline, 3-point circle, angle, rectangle and polygon on live, frozen or opened images; results table; export to Excel/CSV plus the image and an annotated copy.
- Geometry is computed on per-axis calibrated points, so non-square pixels will be handled correctly once Phase 2 provides a calibration.
- To do: hook up calibrations from Phase 2 (`MeasurePanel.set_calibration`).

### Phase 4: Processing tools
- Flat-field correction (divide by a blank reference frame).
- Focus stacking (align frames, pick sharpest pixels by Laplacian).
- HDR via `cv2.createMergeMertens`.
- Stitching via `cv2.Stitcher_create(cv2.Stitcher_SCANS)`.
- Class counting and fluorescence composites.

### Phase 5: GUI (shell done; built ahead of Phases 2–4)
- Done: PySide6 app with live view, camera controls, capture, overlays, freeze and open-file.
- Measurements are drawn in `ImageView.drawForeground` from the panel's list, not as QGraphicsItems. That's simpler, but shapes can't be dragged to edit them; delete and redraw instead.
- To do: add calibration and processing tools as those phases are built.

## Useful open-source tools to compare against

- **Fiji (ImageJ)**: reference for measurement and stitching accuracy
- **focus-stack**: reference for focus-stacking quality
- **napari**: possible GUI base for later
- **Micro-Manager / pycromanager**: scripted acquisition, if automation is needed later

## Notes for Claude (AI assistant context)

- The user runs Windows and develops with Claude Desktop.
- No vendor SDK exists. All camera access goes through OpenCV's standard camera interface. Don't suggest vendor APIs. (HD2's Tucsen `TUCam.dll` also just uses UVC for this camera; see **How HD2 works**.)
- Use DirectShow and set size before FOURCC. Exposure values map to real time as ≈ 10 × 2^value s.
- Set camera controls through `Camera.set()` / `uvc_controls`, not `cv2.CAP_PROP_*`.
- New features go into the PySide6 GUI (`camcontrol/gui/`). Camera access stays on the `CameraWorker` thread.
- Keep modules small and independently runnable (`python -m camcontrol.viewer`), so each tool can be tested alone.
- Measurements must always use per-axis calibration (µm/px in X and Y), never a single scale, until squareness is confirmed.
- Processing tools should accept file paths as input, not only live frames, so full-resolution SD card images can be used.
- Target Python 3.14+ and use PySide6 (not PyQt) for all GUI code.
- Prefer clear, commented code over clever code. The user is building this to learn and to own it.

