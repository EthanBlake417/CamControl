# CamControl (Rust)

The CamControl app rewritten in Rust, built as **one executable** for Windows and Linux, with the same features as the Python app in the folder above. It needs no Python, no installer and no DLLs beyond the ones that come with the system.

Libraries (all MIT, Apache-2.0 or BSD, so they can be linked into the exe with no licence obligations): [egui/eframe](https://github.com/emilk/egui) for the window, [nokhwa](https://github.com/l1npengtul/nokhwa) for camera frames (Media Foundation on Windows, V4L2 on Linux), the `windows` crate for DirectShow camera controls, [image](https://github.com/image-rs/image) and [mozjpeg](https://github.com/kornelski/mozjpeg-sys) for files, [imageproc](https://github.com/image-rs/imageproc) (contours), [openh264](https://github.com/ralfbiedert/openh264-rs) + [mp4](https://github.com/alfg/mp4-rust) for MP4 video, [rust_xlsxwriter](https://github.com/jmcnamara/rust_xlsxwriter) for Excel, [tiny-skia](https://github.com/RazrFalcon/tiny-skia) for drawing on exported images, [rfd](https://github.com/PolyMeilex/rfd) for file dialogs, [trash](https://github.com/Byron/trash-rs) for the Recycle Bin.

## Features

Everything the Python app's README describes (shortcuts are the same):

- **Cameras**: any USB (UVC) camera; Device and Resolution choice (remembered per camera), exposure / gain / contrast / saturation / sharpness sliders. On Windows the controls go through DirectShow, as in the Python app, so auto exposure is switched off and the MC802's quirks are handled (exposure ~ 10 x 2^value s; the "no-flags write first" fix after HD2; long exposures past -3 by adding 2-16 frames). Exposure is set as a time in ms: between the camera's doubling steps the step below is brightened in software (up to 2x), and past its longest any number of frames is added. Linux uses V4L2 controls (exposure in 100 µs units, auto exposure switched to manual).
- **Views**: several at once (Single, Side by side, Stacked, Grid), each with its own camera, image, video or result; Link views; Source menu; Save all as one image; several cameras live at once.
- **Capture**: frame averaging, native or HD2-size (1.7x bicubic) save, TIFF/PNG, numbered names (`name-001.tif`, ...), the capture's details (time, exposure, gain, frames averaged, sizes, calibration, source) written **inside the image** as JSON text (TIFF ImageDescription, PNG "Description" text, JPEG comment; ImageJ/Fiji or exiftool show them) instead of a `.json` beside it (Rust only; `capture::read_image_details` reads them back, or an older capture's `.json`). Opening a file that has them shows them in the status bar (hover for all of them); Link views captures from every camera (`_view1`, `_view2`).
- **Video** (MP4 H.264 or AVI Motion JPEG, real-time frame slots, Stop after) and **time-lapse** (fixed schedule, optional video at the end). Playback bar with frame steps and seeking.
- **Measurements** (line, polyline, circle 3 pt / center, angle, rectangle, polygon) with the table and Excel/CSV export plus image and annotated image; **Fiber axis** (all five methods, Update live, detection settings, export; slow and fast axis angles read out in big green letters in the view's top-left corner, one line per fiber, numbered top to bottom then left to right, from the Fiber count panel's top number). The Python app's Counting panel isn't in the Rust app. Rust-only extras, still to add to the Python app:
  - **Smallest rod** (Fiber axis panel > Show advanced settings, image pixels, "any" = every size tried): Panda rod outlines with a smaller radius are never considered, so dust, bubbles or the core can't be outlined instead of a rod. **Draw** sets it on the view: click a rod's center, then its edge. Each fiber gets it as a share of its own radius (18-40%). The **Method** choice is always shown, just above Show advanced settings; Use circle measurements is the last advanced setting.
  - **Axis lines outside the fiber** (View > Grid, crosshair and fiber axes settings... > Fiber axes tab): each fiber's axis lines drawn only outside the fibers, from the fiber's edge to the edges of the view as the crosshair's are (of the picture on exports and videos), skipping any neighbouring fiber, to compare with the crosshair, the grid or lines in the image without covering the fiber. The same tab chooses which axes to **show** (slow, fast or both: lines, readout, exports, videos) and sets the **slow and fast axis colors and opacity**; the readout writes each axis's angle in its line's color.
  - **Crosshair on the fiber**: View > Grid, crosshair and fiber axes settings... > Crosshair > Centered on: "Fiber center" puts the crosshair where the selected fiber's (else fiber 1's) axis lines cross, with its lines across the whole view; with Axis lines outside the fiber on, the crosshair's lines also stop at every fiber's edge (and there's no ring).
  - **Brightest center**: each fiber's center brightness (0-255: the brightest spot within 15% of the radius of its center, 3x3 px or 2% of the radius averaged, so one hot pixel doesn't count) is in the table and the export. With two or more fibers the brightest is ringed in green and the readout adds "Fiber N is on (brightest center)".
- **Fiber log** panel (Ctrl+6, Rust only): tick **Record each update** and every fiber's slow and fast axis is logged each time the Fiber axis panel measures (Find axes, or each Update live). Laid out top to bottom so it fits a side edge too. The **Summary** (per fiber: n, μ slow and μ fast = mean slow and fast axis, σ = standard deviation) sits above the **Log** (time, update, fiber, slow, fast). **Save to CSV...** after taking data writes the log and, beside it, `..._summary.csv` (or pick Excel for one file with both sheets); **Clear** starts again.
- **Finding fibers** (Rust only): circles found are kept only with a cladding edge round most of them (a step, or for faint fibers an outline), so the curved edge of the lit area or shading with no fiber in view finds nothing; fibers the first search missed are looked for with the lighting evened out (a slope of light across the image), by their edges' shape, and as circles the size of the fibers found (fibers side by side in a groove). Live, the fibers are followed between updates only once two searches agree.
- **Alignment** panel (Ctrl+4, Rust only): **Take readings** (Space) measures every fiber's slow axis the chosen number of times (live: the next updates; a still image: once) and adds each fiber's mean angle and standard deviation to the table, and the angle from the **reference line** if one is drawn (**Draw line**, dashed cyan across the view; the live angles from it are listed too). **Region of interest**: **Draw rectangle** or **Draw circle** and fibers are only looked for inside it (dashed yellow). **Save measurement** writes the table as CSV to the output folder; **Save measurement with photo** also saves the view's image with everything drawn on it and the table (PNG).
- **Fiber count** panel (Ctrl+7, Rust only): how many fibers, numbered from the **top fiber's number** counting **ascending** or **descending** (side by side: from the left), the numbers shown on the fibers, and which one **is on** (the brightest center). **Save image with information** writes the marked-up image (PNG) and the details (JSON) to the output folder.
- **Include markup** (Photo, Video and Time-lapse tabs, Rust only): what's saved has everything shown on the view drawn on (fiber axes and readout, numbers, region, reference line, measurements).
- **Histogram** (bottom of the Controls panel, Rust only): the share of the active view's pixels at each brightness 0-255 (luma), as a percentage or **Logarithmic** (log10 of the percentage); optionally red, green and blue; mean, lowest, highest and the share at 0 and 255 (clipped); hover for a value's numbers; Save... writes each value's percentage and log10 per channel to Excel or CSV. Live views are recounted up to 5 times a second.
- **Processing**: flat-field (live and captures, or the image in a view), focus stack (ECC alignment), HDR (AlignMTB + Mertens, OpenCV's defaults), stitching (scans or panorama), fluorescence composites.
- **Captures panel** with background thumbnails, keyboard stepping, right-click tools, delete to the Recycle Bin; grid and crosshair with their settings window; PyCharm-style panel stripes (drag a button to another edge); **settings files** in the Python app's format (files work in both; Rust adds a `fiber` section with the Fiber axis panel's and the Fiber axes tab's settings). Every panel choice (Update live included) is remembered between sessions.
- Command line: `camcontrol [--index N] [files...]` (camera N in the first view; files open, more than one in new views).

## Differences from the Python app

- **Stitching** uses its own feature matcher (FAST corners, oriented binary descriptors, RANSAC) instead of OpenCV's Stitcher: "scans" fits shift + rotation + scale, "panorama" a perspective (homography), blended with feathered edges. Results can differ slightly from OpenCV's.
- **MP4 videos** are H.264. The Python app's MP4s use the older MPEG-4 Part 2 ("mp4v") codec, which this version can't play (its AVIs play fine).
- **Fiber axis** results match the Python app's to a median 0.016° (worst 1.9°) over 69 test fibers, including the real photos (within 0.06°, same types). Improvements over the Python app:
  - Rods running off the image (fibers at the frame's edge, as in V-groove photos) are scored over the part of their outline that shows, so the rod inside the frame doesn't lose to a wrong circle.
  - A clearly visible core spot breaks near-ties between rod pairs (the rods sit either side of it); it never overrules a clear winner, so a speck of dirt taken for the core does no harm.
  - Update live follows the fibers from frame to frame (re-fitting each cladding edge) instead of searching the whole image every time: about 3x more updates a second.
  - Several fibers in one image are measured at once; about 1.7x faster than before on a three-fiber photo, and faster than the Python app.
  - Fibers on a bright background (lit from the front, or grey images where the cladding is about as bright as its surroundings) are found by their faint edge: when no fiber stands out, the circle search runs again on a smaller, smoother copy using the image's own strongest edges. A lit core that passes for a small fiber is swapped for the faint cladding circle around it (unless a strong circle, such as the microscope's field stop, is there).
  - **Colors by angle** (Fiber axes tab, per axis, on by default): an aim (fast axis 0°, slow axis ±90° by default) and three limits (0.5°, 1°, 2°) with a color each (green, yellow, orange); an axis whose |angle| is within a limit of its aim (the smallest that fits) takes that color, lines and readout angle alike; beyond them all, the axis's own color.
  - **Right-click on an image**: the View menu (live, grid, crosshair, settings, zoom, layout, views, source, panels) for that view.
  - **Apple core** method (and part of Automatic): the bright region round a Panda fiber's core, pinched on the two sides where the rods press on it. Its two mirror axes are the fiber's axes; the one along the rod outlines beside it (even faint ones) is the slow axis, or, with no usable rods, the one through the pinches. Automatic uses it when the rods are too washed out to be clear (grey, front-lit pictures), and to keep the rods when they agree with it but the bright pattern would otherwise be taken for an Elliptical or Bow-tie fiber's stress region. Clear when the core is mirror-symmetric (correlation 0.8 about one axis, 0.3 about the other) and clearly pinched on one pair of sides.
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

For quick builds while trying changes, the `fast` profile skips the slow whole-program optimisation (seconds to a minute after the first build, nearly as fast to run): `cargo build --profile fast` gives `target\fast\camcontrol.exe`.

## Tests

```
cargo test --profile fast --lib
```

(or `--release`, slower to build.) 85 tests, about 40 s, no camera needed, nothing written outside a temporary folder:

- image operations against OpenCV's behaviour, every processing tool, measurement maths, exports, settings files, video write/read/seek;
- the window itself, driven by egui's test harness: opening images, Find axes, measuring by clicking, alignment readings, region and reference line, fiber numbering, views and layouts, processing, the clear-measurements question, settings files; and with a pretend camera: live view, capture, the exposure slider (including long exposures), video recording, time-lapse with its video, live fiber axes, moving a camera to another view.

`cargo run --release --example fiber_compare -- <folder>` compares fiber results with the Python app's on images and a `cases.json` written by a Python script; `cargo run --example probe` lists cameras, opens the first and saves one frame; `cargo run --release --example fiber_one -- <image> [method] [--center]` measures one image with timings (`FIBER_OUT=<png>` draws the result).

Environment variables for testing: `CAMCONTROL_FAKE_CAMERA=1` adds a pretend camera ("Fake camera": a turning Panda fiber at 25 fps, with the MC802's controls), `CAMCONTROL_NO_CAMERAS=1` hides the real ones, `CAMCONTROL_TEST_FOLDER=<dir>` starts with that output folder.

## Not tried yet

- A real camera (the MC802 or the ETN3000): live view, the DirectShow controls, capture, recording. Everything camera-related is tested only with the pretend camera.
- Linux.
