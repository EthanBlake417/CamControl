# CamControl specification

What the software must do, and how we know it does. The README explains how to use it; this file is the checklist. Keep it current: when a feature is added, changed or dropped, update its line here (and the test that checks it).

**Status:** **Done** (works and is checked), **Hardware?** (done, but not yet tried on the real camera or setup it's for), **Planned** (not built yet), **Rust only** (done in camcontrol-rs, still to do in the Python app).
**Verified by:** a test in `tests/` (run `venv\Scripts\python -m pytest`), a module self-test (`python -m ...`, also run by the tests), or **Manual** (checked by hand in the app).

Owner: Ethan Blake, Grid Evolution Technologies. Last reviewed: 2026-10-23 (Python brought level with the Rust app).

## 1. Purpose and scope

CamControl is a Windows desktop app for live viewing, capture, measurement and image processing with USB microscope cameras. It replaces the parts of the vendor software (O.C. White "HD2 Advanced Imaging") used day to day, and adds tools HD2 doesn't have, mainly measuring the axes of polarization-maintaining (PM) fibers.

Cameras it's for: the O.C. White Ultra-Cam II (Windows name "MC802 USB2.0 Camera") on a MacroZoom lens or microscope, and the ETN3000 fiber end-face inspector. Any standard USB (UVC) camera should work.

## 2. Platform

| ID | Requirement | Status | Verified by |
|---|---|---|---|
| PLAT-1 | Runs on Windows 10/11 (64-bit). | Done | Manual |
| PLAT-2 | Built with Python 3.14, PySide6 (Qt), OpenCV, numpy, pandas/openpyxl, comtypes; environment managed with uv in `venv/`. | Done | `uv sync` |
| PLAT-3 | Needs no vendor SDK or driver: cameras are used through Windows' standard UVC driver (DirectShow). | Done | Manual |
| PLAT-4 | Runs as a standalone Windows program (`.exe` plus its folder), without Python installed. | Done | (see section 16) |

## 3. Cameras

| ID | Requirement | Status | Verified by |
|---|---|---|---|
| CAM-1 | Lists every camera Windows has and opens the chosen one; remembers it by name (and which one, if two share a name). | Done | `test_views.py` |
| CAM-2 | Opens a camera at its largest frame size, MJPG where offered (frame size set before MJPG, else DirectShow drops to YUY2 at ~6 fps). Resolution can be changed and is remembered per camera. | Done | Manual (MC802), Hardware? (ETN3000) |
| CAM-3 | Sliders for exposure, gain, contrast, saturation, sharpness, set through the Windows camera interfaces directly (not OpenCV). Controls a camera lacks show "not available". | Done | Manual, `test_views.py` |
| CAM-4 | Recovers exposure control after HD2 has used the camera (a write without the "manual" flag first). | Hardware? | Manual (not re-checked since 2026-09) |
| CAM-5 | Exposure beyond the camera's maximum (to ~20 s) by adding frames in software; live view shows a running sum; captures record `frames_summed_per_exposure`. | Hardware? | Fake camera only |
| CAM-9 | Exposure set as a time in ms (typed, with decimals, or a logarithmic slider), not only whole camera steps. Windows: the camera's step just below, brightened in software up to 2x; past its longest, any number of frames added (up to 16). Linux: 0.1 ms steps. Saved as `exposure_us` beside `exposure` in settings files. | Hardware? | `test_parts.py`, `test_views.py` (fake camera), `cargo test` |
| CAM-6 | All camera access runs on a background thread per camera, so the window never waits for a camera. Frames are dropped, not queued, if the window falls behind. | Done | `test_views.py` |
| CAM-7 | A camera that's off, unplugged or stops sending shows "Not connected"; **Reconnect** opens it again. | Done | Manual |
| CAM-8 | Two or more cameras live at once, each in its own view on its own thread. | Hardware? | `test_views.py` (fake cameras); real pair not tried |

## 4. Views

| ID | Requirement | Status | Verified by |
|---|---|---|---|
| VIEW-1 | One view looks and works like a single-image app (no extra bars). | Done | `test_views.py` |
| VIEW-2 | Up to 12 views, each with its own source: a camera, an image file, a video or a processing result. | Done | `test_views.py` |
| VIEW-3 | Layouts: Single (tabs to switch), Side by side, Stacked, Grid (columns chosen to show the views largest). Ctrl+K switches between one view and several. | Done | `test_views.py` |
| VIEW-4 | Each view has a title (what it shows) and a Source menu: pick a camera, No camera, Live, Open image, Remove. A camera in another view moves to this one. | Done | `test_views.py` |
| VIEW-5 | The view clicked last is active (highlighted). Measurements, counting, fiber axes, open image, results and save go to it; each view keeps its own measurements, marks and axes. | Done | `test_views.py` |
| VIEW-6 | Controls work on the active view's camera, or, if it has none, on the camera used last; the Camera box says which view. | Done | `test_views.py` |
| VIEW-7 | A camera view showing a file keeps its camera running; Live (L) goes back to it. | Done | `test_views.py` |
| VIEW-8 | Link views (on by default): zoom/pan follow across views (matched by position in the picture); Capture, Live/Freeze, Fit, 100% and Find fiber axes act on every view. Off: on the active view only. | Done | `test_views.py` |
| VIEW-9 | Save all views as one image, with their names. | Done | Manual |
| VIEW-10 | Layout, Link views and each view's camera or file are remembered, and saved in settings files. Settings from before views (one camera) still load. | Done | `test_views.py` |

## 5. Display and overlays

| ID | Requirement | Status | Verified by |
|---|---|---|---|
| DISP-1 | Zoom (wheel, Ctrl+=/-, Fit, 100%) and pan (drag). Smooth when zoomed out, sharp pixels from 4x up. | Done | Manual |
| DISP-2 | Status bar: pixel position and RGB/value under the mouse, zoom, image size, frame rate, and what the active view shows. | Done | Manual |
| OVL-1 | Grid overlay on every view: columns and rows, or square cells; line width, style, color, opacity; same on-screen width at any zoom. | Done | `test_parts.py`, `test_views.py` |
| OVL-2 | Crosshair overlay on every view: on the image centre or the window centre; ring size; line width, style, color, opacity. | Done | `test_parts.py`, `test_views.py` |
| OVL-4 | Crosshair centre "Fiber centre": where the selected fiber's (else fiber 1's) axis lines cross, lines across the whole view; with "Axis lines outside the fiber" on, the lines stop at every fiber's edge and there's no ring | Done | `test_views.py`, `cargo test` |
| OVL-5 | Histogram panel (Ctrl+7): % of the active view's pixels at each brightness 0-255 (luma), linear or log10; optional R, G, B; mean, lowest, highest, share at 0 and 255; Save as Excel/CSV (value, % and log10(%) per channel). Live views recounted up to 5 times a second. | Done | `test_parts.py`, `cargo test` |
| OVL-6 | Right-click on an image: the View menu, acting on that view. | Done | `test_views.py`, `cargo test` |
| OVL-3 | Overlay settings window stays open and the overlays follow as it's changed; settings remembered and in settings files. | Done | `test_views.py` |

## 6. Capture and output

| ID | Requirement | Status | Verified by |
|---|---|---|---|
| CAP-1 | Capture (Space) with frame averaging (1-32 frames), save size (native or HD2's 1.7x), TIFF or PNG. | Done | `test_views.py`, Manual |
| CAP-2 | Never overwrites: `name-001`, `name-002`, ... continuing after the highest number in the folder (any extension); blank name gives date-time names. The next name is shown. | Done | `test_parts.py` |
| CAP-3 | Each saved image carries its settings (exposure, gain, frames averaged, size, upscaling, flat-field, source) inside it (TIFF ImageDescription, PNG Description text, JPEG comment), no `.json`; older `.json` files still read; shown in the status bar when such a file is opened (hover: all of them). Files from either app read in the other | Done | `test_parts.py`, `test_views.py`, `cargo test` |
| CAP-4 | Capturing from several cameras at once (Link views) saves each in its own series (`name_view1-001`, `name_view2-001`). | Done | `test_views.py` |
| CAP-5 | Save image as (Ctrl+S) saves what the active view shows: a result, a frozen frame, a paused video frame (frame number recorded). | Done | `test_views.py` |

## 7. Video and time-lapse

| ID | Requirement | Status | Verified by |
|---|---|---|---|
| REC-1 | Record the live view to MP4 or AVI (MJPG), optionally stopping after a set time, at real speed whatever the camera's frame rate. | Done | `camcontrol.recording` self-test, Manual |
| REC-2 | Time-lapse: a capture every N s/min/h, a set number or until stopped, optionally joined into a video. | Done | `camcontrol.recording` self-test, Manual |
| REC-3 | Play MP4/AVI in a view: play/pause, frame steps, seek, time and frame number. | Done | `test_views.py` |
| REC-4 | Closing the app, reconnecting or changing a camera while recording asks first; a recording is always left playable. | Done | Manual |
| REC-5 | "Draw fiber axes on the video" (on by default): recorded frames carry the view's latest fiber axes and the angle readout, as on screen. | Hardware? | `test_parts.py`, `test_views.py` (fake camera), `cargo test` |

## 8. Captures panel

| ID | Requirement | Status | Verified by |
|---|---|---|---|
| GAL-1 | Thumbnails of the newest N images and videos in the output folder (or another folder chosen), updating when files change. | Done | `test_parts.py` |
| GAL-2 | Thumbnails load in the background; a big or slow folder doesn't hold up the window. | Done | `test_parts.py` |
| GAL-3 | Double-click opens; arrow keys/Home/End/PgUp/PgDn step through and open. The open image is outlined, selected ones tinted. | Done | `test_parts.py`, Manual |
| GAL-4 | Right-click: open in new views, show in Explorer, processing tools, delete (to the Recycle Bin, with its `.json` if it has one (older files), after asking) | Done | Manual |

## 9. Measurement and calibration

| ID | Requirement | Status | Verified by |
|---|---|---|---|
| MEAS-1 | Tools: line, polyline, circle (3 points), circle (centre and edge), angle, rectangle, polygon (area and perimeter). | Done | `camcontrol.measure` self-test |
| MEAS-2 | Results table; selecting a row highlights the shape; Backspace/Esc/middle-drag while drawing. | Done | Manual |
| MEAS-3 | Export: table (`.xlsx` with an Info sheet, or `.csv`), the image, and an annotated copy. | Done | Manual |
| CAL-1 | Calibration table: µm per pixel per zoom setting, X and Y separately, saved to JSON; measurements shown in µm. Records the image size it applies to (HD2-size files are 1920/3264 of native). | Planned | |

## 10. Counting

| ID | Requirement | Status | Verified by |
|---|---|---|---|
| CNT-1 | Up to 5 named classes; click to add, right-click to remove the nearest, Backspace to undo; counts and percentages live. | Done | `camcontrol.processing.count` self-test |
| CNT-2 | Export: counts (`.xlsx` Summary/Marks/Info, or `.csv`), the image and a marked copy. | Done | Manual |

## 11. Fiber axis

| ID | Requirement | Status | Verified by |
|---|---|---|---|
| FIB-1 | Finds fibers automatically, including touching fibers and fibers running off the image; or uses circles drawn in Measurements. "Only the centre fiber" option. | Done | `camcontrol.processing.fiber_axis` self-test |
| FIB-2 | Slow axis (through the stress parts) and fast axis (90°), angles −90° to +90° (0 = horizontal, + = counter-clockwise), type (Panda, Bow-tie, Elliptical) and clarity; "(unclear)" when unsure. | Done | `test_processing.py` |
| FIB-3 | Automatic method: Panda rod outlines for Panda fibers, mirror symmetry for Bow-tie and Elliptical. Other methods available in advanced settings. | Done | `test_processing.py` |
| FIB-4 | Robust to dirt: dust, bubbles, debris clusters, black streaks beside a rod, a blob over a rod, shading, embossed (relief) rods, a glow round the cladding. | Done | `test_processing.py` |
| FIB-5 | Accuracy on synthetic dirty Panda fibers: median error under 0.3°, at least 85% within 1°, none marked clear more than 5° off. (Today: median 0.09-0.18°, 94-95% within 1°.) | Done | `test_processing.py` (slow) |
| FIB-6 | Real end faces: ETN3000 photos within 0.6° of the reference; the 36879B bow-tie TIFs measured as Bow-tie. | Done | `test_processing.py` (when the files are present) |
| FIB-7 | Update live: re-measures live views about 3 times a second (taking turns between views, following the fibers found last time), each image as it's opened, and a video's frames while it plays (as often as the measuring keeps up) or the frame stepped to | Hardware? | `test_views.py` (fake cameras), `test_processing.py` (following) |
| FIB-8 | Show stress parts (shaded outlines, centres, pivot); export table, image and a drawn copy. | Done | Manual |
| FIB-9 | Angle readout in big letters (26 px on screen; scaled with the image on the drawn copy and videos) in the top-left corner of each view: one line per fiber, numbered left to right, "1: slow +0.5°  fast -89.5°" with each angle in its axis line's color and the number in the first one's; only the axes chosen in Show, "(unclear)" when unsure; no number tag on the fibers themselves | Done | `test_parts.py`, `cargo test`, Manual |
| FIB-10 | Smallest rod (image pixels, "any" = every size; under Show advanced settings): Panda rod outlines with a smaller radius aren't considered; "Draw" sets it by clicking a rod's centre then its edge on the view; each fiber gets it as 18-40% of its radius. Saved in the fiber detection settings as `min_rod_px` (`min_rod` % still read when no pixels are set) | Done | `test_processing.py`, `test_views.py`, `cargo test` |
| FIB-11 | "Outside the fiber" (View > Grid, crosshair and fiber axes settings... > Fiber axes tab): axis lines only outside the fibers, from the fiber's edge to the view's edges like the crosshair (the picture's edges on the drawn copy and videos), skipping neighbouring fibers. Same tab: Show slow, fast or both axes (lines, readout, exports, videos); slow and fast axis line color and opacity (also the readout's angle colors). The panel: Find axes + Clear; Method always shown above Show advanced settings; advanced: Smallest rod, Turn about stress parts, Use circle measurements (export is in the Measure menu) | Done | `test_parts.py`, `test_views.py`, `cargo test`, Manual |
| FIB-12 | Brightest centre: each fiber's centre brightness (brightest small-box average within 15% of the radius of the centre) in the table and export; with 2+ fibers the brightest is ringed in green and the readout adds "Brightest centre: position N from right (fiber k)". | Done | `test_processing.py`, `cargo test` (synthetic + the real V-groove photo by eye) |
| FIB-13 | Fiber log panel: "Record each update" logs every fiber's slow and fast axis per update; per-fiber summary (count, mean slow, mean fast, std) above the log; "Save to CSV..." afterwards (log + `_summary.csv`, or one Excel file); Clear.; laid out top to bottom (controls, summary, log) so it fits a side edge as well as the bottom; summary headings Fiber, n, μ slow °, μ fast °, σ ° | Done | `test_parts.py`, `test_views.py`, `cargo test` (stats), Manual |
| FIB-14 | Faint-edge fibers: fibers on a bright/grey background (cladding about as bright as the surroundings) found by a second circle search on a smoother copy with the image's own strongest edges; a lit core taken for a small fiber is replaced by the faint cladding circle around it. | Done | `test_processing.py`, `cargo test`, the 2026-10-20 grey captures by eye |
| FIB-15 | Apple core method ("core"): the pinched bright region round a Panda's core; of its two mirror axes, the one along the rod outlines beside it (even faint, within 10°) is slow, else the one through the pinches; clear only if symmetric and pinched. Automatic uses it when the rods are unclear, and keeps the rods instead of switching to Elliptical/Bow-tie when rods and core agree (within 3°) and the other shape doesn't | Done | `test_processing.py`, `cargo test` (drawn apple cores; the 2026-10-20 grey captures when present: Python 16 of 17, 181407 comes out unclear at the right angle) |
| FIB-16 | Colors by angle (Fiber axes tab, per axis, on by default): an aim (fast 0°, slow ±90°) and 3 limits (0.5°, 1°, 2°) with a color each (green, yellow, orange); an axis whose |angle| is within a limit of its aim (smallest that fits) takes its color, lines and readout angle; else the axis color | Done | `test_parts.py`, `cargo test` |

## 12. Processing

| ID | Requirement | Status | Verified by |
|---|---|---|---|
| PROC-1 | Flat-field correction: take or load a reference; correct live views, captures, videos and time-lapses (all cameras), or an opened image. | Done | `camcontrol.processing.flatfield` self-test, `test_views.py` |
| PROC-2 | Focus stacking (with alignment). | Done | `camcontrol.processing.focus_stack` self-test |
| PROC-3 | HDR exposure fusion. | Done | `camcontrol.processing.hdr` self-test |
| PROC-4 | Stitching overlapping images. | Done | `camcontrol.processing.stitch` self-test |
| PROC-5 | Fluorescence composite (channel colors guessed from file names, editable). | Done | `camcontrol.processing.fluorescence` self-test |
| PROC-6 | Results show unsaved in the active view; saving keeps the inputs and settings inside the image. Works on any image files, including HD2 and SD-card sizes | Done | `test_views.py` |

## 13. Settings and persistence

| ID | Requirement | Status | Verified by |
|---|---|---|---|
| SET-1 | Everything set in the app (window layout, panels, folders, options, views, overlays, fiber settings) is remembered between runs. | Done | `test_views.py` |
| SET-2 | Settings files (JSON, named by the user, `settings/` by default): camera values per camera (incl. `exposure_us`), capture, output, flat-field, recording, overlays, views, and a `fiber` section (method, Update live, show parts, centre fiber, circle measurements, turn about parts, detection incl. smallest rod, outside lines, line colors and colors by angle). Same format in both apps. The last one opens at start-up | Done | `test_parts.py`, `test_views.py` |
| SET-4 | Remembered between sessions (QSettings; Rust: app storage): every panel choice, including Update live, Use circle measurements, the Fiber log's Record each update, the Histogram's options, counting class names, the last several-views layout, the overlay window's tab and the fiber line settings. | Done | `test_views.py`, `cargo test` |
| SET-3 | Older settings and settings files keep loading after changes (unknown or bad values are ignored). | Done | `test_parts.py`, `test_views.py` |

## 14. User interface

| ID | Requirement | Status | Verified by |
|---|---|---|---|
| UI-1 | Panels (Controls, Captures, Measurements, Counting, Fiber axis, Fiber log, Histogram) on edge stripes like PyCharm: one open per edge, minimize, drag to another edge; Ctrl+1..7. | Done | Manual |
| UI-2 | Keyboard shortcuts as listed in the README. | Done | Manual |
| UI-3 | Help → About: what the app does, the GET logo, "Made by Ethan Blake at Grid Evolution Technologies." | Done | Manual |
| UI-4 | Window and taskbar icon (GET logo). | Hardware? | Manual (taskbar icon not confirmed) |

## 15. Performance and reliability

| ID | Requirement | Status | Verified by |
|---|---|---|---|
| PERF-1 | The window stays responsive: no freeze over about 50 ms during live video, fiber measuring, setting changes or browsing images. (Measured: 5-25 ms.) | Done | Manual (offscreen timing), `test_views.py` |
| PERF-2 | Fiber axis of one fiber in about 0.25 s at 1920x1080, in the background. | Done | Manual |
| PERF-3 | Showing a 1080p frame costs about 4 ms of the window's time, so two 30 fps cameras use about a quarter of it. | Done | Manual |
| REL-1 | No data loss: captures never overwrite; deletions go to the Recycle Bin; recordings are finished cleanly on close. | Done | `test_parts.py`, Manual |
| REL-2 | Results that no longer match what's shown (old settings, another image) are dropped, never displayed. | Done | `test_views.py` |
| REL-3 | Tests never write outside a temporary folder. | Done | `tests/conftest.py` |

## 16. Out of scope (for now)

- Replacing the camera's own HDMI/SD-card interface.
- RAW or 16-bit capture, or anything needing a vendor driver or SDK (none is published).
- Calibration until a stage micrometer is available (CAL-1 is planned).
- Platforms other than Windows.
