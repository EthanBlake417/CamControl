# CamControl (Rust)

The CamControl app rewritten in Rust, built as **one executable** for Windows and Linux, with the same features as the Python app in the folder above. It needs no Python, no installer and no DLLs beyond the ones that come with the system.

Libraries (all MIT, Apache-2.0 or BSD, so they can be linked into the exe with no licence obligations): [egui/eframe](https://github.com/emilk/egui) for the window, [nokhwa](https://github.com/l1npengtul/nokhwa) for camera frames (Media Foundation on Windows, V4L2 on Linux), the `windows` crate for DirectShow camera controls, [image](https://github.com/image-rs/image) and [mozjpeg](https://github.com/kornelski/mozjpeg-sys) for files, [imageproc](https://github.com/image-rs/imageproc) (contours), [openh264](https://github.com/ralfbiedert/openh264-rs) + [mp4](https://github.com/alfg/mp4-rust) for MP4 video, [rust_xlsxwriter](https://github.com/jmcnamara/rust_xlsxwriter) for Excel, [tiny-skia](https://github.com/RazrFalcon/tiny-skia) for drawing on exported images, [rfd](https://github.com/PolyMeilex/rfd) for file dialogs, [trash](https://github.com/Byron/trash-rs) for the Recycle Bin.

## Features

Everything the Python app's README describes (shortcuts are the same):

- **Cameras**: any USB (UVC) camera; Device and Resolution choice (remembered per camera), exposure / gain / contrast / saturation / sharpness sliders. On Windows the controls go through DirectShow, as in the Python app, so auto exposure is switched off and the MC802's quirks are handled (exposure ~ 10 x 2^value s; the "no-flags write first" fix after HD2; long exposures past -3 by adding 2-16 frames). Exposure is set as a time in ms: between the camera's doubling steps the step below is brightened in software (up to 2x), and past its longest any number of frames is added. Linux uses V4L2 controls (exposure in 100 µs units, auto exposure switched to manual).
- **Views**: several at once (Single, Side by side, Stacked, Grid), each with its own camera, image, video or result; Link views; Source menu; Save all as one image; several cameras live at once.
- **Capture**: frame averaging, native or HD2-size (1.7x bicubic) save, TIFF/PNG, numbered names (`name-001.tif`, ...), `.json` sidecar with the same keys; Link views captures from every camera (`_view1`, `_view2`).
- **Video** (MP4 H.264 or AVI Motion JPEG, real-time frame slots, Stop after) and **time-lapse** (fixed schedule, optional video at the end). Playback bar with frame steps and seeking.
- **Measurements** (line, polyline, circle 3 pt / centre, angle, rectangle, polygon) with the table and Excel/CSV export plus image and annotated image; **Counting** (5 classes); **Fiber axis** (all five methods, Update live, detection settings, export; angles read out in big green letters in the view's top-left corner; videos can be recorded with the axes drawn on). Rust-only extras, still to add to the Python app:
  - **Smallest rod** (Fiber axis panel, % of the fiber radius, default 18% = every size tried): Panda rod outlines smaller than this are never considered, so dust, bubbles or the core can't be outlined instead of a rod.
  - **Axis lines across the view**: each fiber's axis lines carried on, thinner, to the edges of the view (and of exports and videos), to compare with the crosshair, the grid or lines in the image.
  - **Crosshair on the fiber**: View > Overlay settings > Crosshair > Centre: "Fiber centre" puts the crosshair on the selected fiber (else fiber 1), with its lines across the whole view.
  - **Brightest centre**: each fiber's centre brightness (0-255: the brightest spot within 15% of the radius of its centre, 3x3 px or 2% of the radius averaged, so one hot pixel doesn't count) is in the table and the export. With two or more fibers the brightest is ringed in green and the readout adds "Brightest centre: position N from right (fiber k)" (position 1 = rightmost; fibers are numbered left to right).
- **Histogram** panel (Ctrl+7, Rust only): the share of the active view's pixels at each brightness 0-255 (luma), as a percentage or **Logarithmic** (log10 of the percentage); optionally red, green and blue; mean, lowest, highest and the share at 0 and 255 (clipped); hover for a value's numbers; Save... writes each value's percentage and log10 per channel to Excel or CSV. Live views are recounted up to 5 times a second.
- **Processing**: flat-field (live and captures, or the image in a view), focus stack (ECC alignment), HDR (AlignMTB + Mertens, OpenCV's defaults), stitching (scans or panorama), fluorescence composites.
- **Captures panel** with background thumbnails, keyboard stepping, right-click tools, delete to the Recycle Bin; grid and crosshair with their settings window; PyCharm-style panel stripes (drag a button to another edge); **settings files** in the Python app's format (files work in both).
- Command line: `camcontrol [--index N] [files...]` (camera N in the first view; files open, more than one in new views).

## Differences from the Python app

- **Stitching** uses its own feature matcher (FAST corners, oriented binary descriptors, RANSAC) instead of OpenCV's Stitcher: "scans" fits shift + rotation + scale, "panorama" a perspective (homography), blended with feathered edges. Results can differ slightly from OpenCV's.
- **MP4 videos** are H.264. The Python app's MP4s use the older MPEG-4 Part 2 ("mp4v") codec, which this version can't play (its AVIs play fine).
- **Fiber axis** results match the Python app's to a median 0.016° (worst 1.9°) over 69 test fibers, including the real photos (within 0.06°, same types). Improvements over the Python app:
  - Rods running off the image (fibers at the frame's edge, as in V-groove photos) are scored over the part of their outline that shows, so the rod inside the frame doesn't lose to a wrong circle.
  - A clearly visible core spot breaks near-ties between rod pairs (the rods sit either side of it); it never overrules a clear winner, so a speck of dirt taken for the core does no harm.
  - Update live follows the fibers from frame to frame (re-fitting each cladding edge) instead of searching the whole image every time: about 3x more updates a second.
  - Several fibers in one image are measured at once; about 1.7x faster than before on a three-fiber photo, and faster than the Python app.
  - Python crashes on a plain disc with no rods (the rod radius goes negative); here it doesn't.
- The default output folder is `Pictures\CamControl` (the program's own folder may not be writable). Settings are kept by the window system's app storage, not QSettings.

## Build

Windows (Rust, and Visual Studio's C++ build tools for the bundled C code):

```
cargo build --release
```

The result is `target\release\camcontrol.exe` (about 13 MB, with the GET icon). It needs only DLLs that come with Windows (the C runtime is linked in, see `.cargo/config.toml`). Windows "N" editions need the Media Feature Pack for cameras.

Linux (build on the oldest distro you want to support, e.g. Ubuntu 22.04):

```
sudo apt install build-essential pkg-config libclang-dev
cargo build --release
```

The result is `target/release/camcontrol`. It runs on X11 and Wayland; file dialogs use the desktop portal (`xdg-desktop-portal`, standard on desktop installs). **Not built or tried on Linux yet.**

## Tests

```
cargo test --release
```

63 tests, about 6 s, no camera needed, nothing written outside a temporary folder:

- image operations against OpenCV's behaviour, every processing tool, measurement maths, exports, settings files, video write/read/seek;
- the window itself, driven by egui's test harness: opening images, Find axes, measuring by clicking, counting, views and layouts, processing, the clear-measurements question, settings files; and with a pretend camera: live view, capture, the exposure slider (including long exposures), video recording, time-lapse with its video, live fiber axes, moving a camera to another view.

`cargo run --release --example fiber_compare -- <folder>` compares fiber results with the Python app's on images and a `cases.json` written by a Python script; `cargo run --example probe` lists cameras, opens the first and saves one frame; `cargo run --release --example fiber_one -- <image> [method] [--centre]` measures one image with timings (`FIBER_OUT=<png>` draws the result).

Environment variables for testing: `CAMCONTROL_FAKE_CAMERA=1` adds a pretend camera ("Fake camera": a turning Panda fiber at 25 fps, with the MC802's controls), `CAMCONTROL_NO_CAMERAS=1` hides the real ones, `CAMCONTROL_TEST_FOLDER=<dir>` starts with that output folder.

## Not tried yet

- A real camera (the MC802 or the ETN3000): live view, the DirectShow controls, capture, recording. Everything camera-related is tested only with the pretend camera.
- Linux.
