# CamControl

Python application for live viewing, capture, and calibrated measurement with a USB microscope camera. Built as a replacement for the parts of vendor imaging software (O.C. White "HD2 Advanced Imaging") used day to day.

## Status

Early development. Done: camera probe (Phase 0), live view and capture (Phase 1), measurement in pixels (Phase 3), processing tools (Phase 4) and the GUI (Phase 5). Next: calibration (Phase 2), so measurements can be in µm.

**GUI** (main app). Use PyCharm's Run button on `main.py`, or:

```powershell
venv\Scripts\python main.py
```

The GUI has a live image with zoom (mouse wheel) and pan (drag); sliders for exposure, gain, contrast, saturation and sharpness; capture with frame averaging, save size, format and folder; grid and crosshair overlays; freeze; opening image files; and a pixel readout (x, y, RGB) in the status bar. Settings are remembered between runs.

| Key | Action |
|---|---|
| Space | capture |
| `L` | live / freeze |
| `G` / `C` | grid / crosshair |
| `F` / `1` | fit to window / 100% |
| Ctrl+= / Ctrl+- | zoom in / out |
| Ctrl+O | open an image file |
| Ctrl+S | save the image in the view (e.g. a processing result, or a paused video frame) |
| Ctrl+Shift+S | save settings (File → Save settings) |
| Ctrl+E | export measurements |
| Ctrl+K | compare images in a grid |
| Ctrl+1 / 2 / 3 / 4 / 5 | show / hide the Controls / Measurements / Captures / Counting / Fiber axis panel (also **View → Panels**, or the panel's × button). Side panels always run the full height of the window. |

**Panels** work like PyCharm's tool windows. Each panel has a button with its name on a thin stripe along the edge it's on (left, right or bottom). Each edge shows one panel at a time: clicking a button opens that panel and minimizes the others on the same edge. Click it again, or the panel's **–** button, to minimize it so the image gets its space. Highlighted buttons are open panels. To move a panel, drag its button to another stripe (all three show while dragging, even empty ones), or along its stripe to change the order.

**Controls panel**, top to bottom: **Camera** (connection status, **Reconnect**, **Device**, **Resolution**, sliders); **Output** (folder, name, next file name, flat-field on/off, shared by everything that saves files); then tabs for **Photo** (averaging, save size, format, Capture), **Video** and **Time-lapse**; and **View**. If the camera was off or unplugged when the app started, or it stops sending pictures, turn it on and click **Reconnect** (also **File → Reconnect camera**).

**Other cameras.** Any standard USB (UVC) camera Windows lists works, e.g. the ETN3000 fiber end-face inspector. Pick it in **Device**; the app remembers it by name and opens it at its largest frame size (change that in **Resolution**, remembered per camera). Sliders for controls a camera doesn't have show "not available". Save size follows the camera: native, or 1.7x upscaled (HD2 size, 3264x1836 for a 1080p camera). The exposure-time estimates (and long exposures past the camera's maximum) were measured on the MC802 and may be off for other cameras. `python -m camcontrol.uvc_controls` lists every camera with its sizes and formats.

**Capturing.** Set a **Name** in the Output box and captures are saved as `name-001.tif`, `name-002.tif`, ..., continuing after the highest number already in the folder (so nothing is overwritten; same style as HD2). Leave it blank for date-and-time names. The line under the field shows the next file name. Each image gets a `.json` with its settings. The **Captures** panel shows thumbnails of the newest N images and videos in the capture folder (N is set in the panel; videos have a play mark). To look at a different folder, click **...** next to **Folder** in the panel. This turns off **Same as output folder**, so captures still save to the Output box folder. Tick it again to go back. The panel remembers the folder. To look at a different folder, click **...** next to **Folder** in the panel. This turns off **Same as output folder**, so captures still save to the Output box folder. Tick it again to go back. The panel remembers the folder. It updates automatically when files change. Double-click a thumbnail to open it, or to play a video; then the arrow keys (also Home, End, Page Up and Page Down) step to the next or previous one and open it, so you can click through the folder (with Shift or Ctrl held they only select). The image open in the main view has an orange frame and a bold name; selected ones are shaded blue. Thumbnails are read in the background, so a big or slow folder doesn't hold up the window; right-click for "Show in Explorer". Ctrl- or Shift-click selects several, and right-clicking them offers the processing tools. **Delete** (right-click, or the Delete key) moves the selected files and their `.json` settings files to the Recycle Bin, after asking. A `.json` shared by two images of the same name (e.g. `name-001.tif` and `name-001.png`) goes when the last of them does.

**Videos** (MP4/AVI, e.g. from the Video tab) play in the main view with a bar underneath: play/pause, previous/next frame, a slider to jump, and the time and frame number. A paused frame can be zoomed, measured, counted or saved (Ctrl+S; its `.json` records the frame number). **Close video**, or Live, goes back to the camera. The right end of the status bar shows what the view is showing: **Live**, **Frozen**, or the open file's name (hover for the full path).

**Measuring.** Pick a tool in the Measurements panel (Line, Polyline, Circle (3 pt), Circle (centre), Angle, Rectangle, Polygon) and click points on the image. Line, circle, angle and rectangle finish by themselves. **Circle (3 pt)** goes through three points on the edge; **Circle (centre)** starts at the centre: click it, then click the edge (the circle follows the mouse in between), or press at the centre and drag out to the edge. For a polyline or polygon, double-click, right-click or press Enter to finish. Backspace removes the last point, Esc cancels (press it twice to go back to panning), and middle-drag pans while a tool is active. Results appear in the table, and selecting a row highlights that shape. **Export** writes the table (`.xlsx` with an Info sheet, or `.csv`), the measured image (`_image.png`) and an annotated copy (`_annotated.png`). Freeze the live view (L) or open a file before measuring, so the image doesn't change underneath you. Until calibration exists, all values are in pixels.

**Counting.** In the Counting panel (a tab next to Measurements), name up to 5 classes, pick one, and press **Count**. Each left-click on the image adds a mark of that class; right-click removes the nearest mark, Backspace undoes the last one, and Esc stops counting. Counts and percentages update as you click. **Export** writes the counts (`.xlsx` with Summary, Marks and Info sheets, or `.csv` plus `_marks.csv`), the image and a copy with the marks drawn on (`_marked.png`).

**Settings files** (File menu: **Open settings**, **Open recent settings**, **Save settings** (Ctrl+Shift+S), **Save settings as**). Saves the whole setup to a `.json` file you name, e.g. `brightfield 10x.json`: camera sliders, Photo averaging/size/format, Output folder and name, flat-field reference and whether it's on, Video/Time-lapse settings, and grid/crosshair. They're saved to `settings/` by default, but can go anywhere and be copied to another PC. The most recently opened or saved file opens again when the app starts, and its name shows in the window title. Camera values in a file opened before the camera connects are applied as soon as it does. To delete a settings file, delete it in Explorer.

**Compare** (View → Compare images, Ctrl+K, or select images in Captures and right-click → Add to compare). Shows saved images in a grid with the main view, which can be live, frozen, a file or a result. **Add images...** adds more (up to 11, several at once), and the **×** above each one removes it. **Remove all** clears them; **Close** hides the grid but keeps the images for next time. The grid picks the number of columns that shows the images largest in the space there is, and every image gets the same space under its own name. Click an image to measure, count or find fiber axes on it: each image keeps its own measurements, marks and axes, and the highlighted name shows which one the panels are working on. With **Link zoom/pan** on, zooming or panning any image does the same to all the others. Images of different sizes are matched by position in the picture. **Save grid** writes all the images in one file, at the same height, with their names on them.

**Video and time-lapse** (the Video and Time-lapse tabs in Controls). **Video** records the live view to MP4 (small) or AVI (Motion JPEG: better quality, much larger), optionally stopping after a set time. Frames are repeated or skipped to match the chosen frame rate, so the video plays at real speed even though the camera's rate varies. **Time-lapse** takes a capture every N seconds/minutes/hours, for a set number of images or until stopped. It uses the Photo tab's averaging, size and format, and a `.json` for each image records its number and the interval. Optionally the images are joined into a video at the end. Files are saved to the Output folder with the Output name, continuing its numbering. Flat-field correction applies to both if it's on. Closing the app asks first while either is running.

**Fiber axis** (panel at the bottom, Ctrl+5; also Measure → Find fiber axes). For polarization-maintaining fiber end faces (Panda, Bow-tie, Elliptical-clad), **Find axes** finds each fiber and draws its slow axis (solid magenta, through the stress rods / wedges / ellipse) and fast axis (dashed yellow, at 90°). The table lists the type, both angles (0° = horizontal, + = turned counter-clockwise, − = clockwise, from −90° to +90°), how clear the axis is, the centre and the diameter. Fibers are found automatically, also when they touch each other (e.g. side by side in a V-groove) or run off the image; **Only the centre fiber** measures just the one nearest the middle of the image (e.g. the lit fiber in a V-groove) and ignores its neighbours. If a fiber isn't found, draw a **Circle (3 pt)** or **Circle (centre)** round it in Measurements and click Find axes again (with "Use circle measurements" on, only circled fibers are measured). **Update live** keeps measuring the live view about 3 times a second (one fiber takes about 0.25 s), e.g. while rotating a fiber into line, and measures each saved image as you open it (e.g. clicking through Captures). All measuring (also Find axes and changing a setting) runs in the background, one at a time, so the video and the window keep responding: frames that arrive while it is busy are skipped, only the newest image and settings are measured next, and a result measured with settings since changed, or for an image no longer shown, is dropped. Opening another image clears the axes of the last one. **Export** writes the table (Excel/CSV), the image and a copy with the axes drawn. Normally nothing needs setting: the fiber type is worked out for each fiber, and dust, bubbles, dirt streaks and shading on the end face are allowed for. **Clarity** says how sure the result is; "(unclear)" means check it by eye (e.g. a rod hidden under dirt, or cut off by the image edge). **Show advanced settings** shows the **Method** box: **Automatic** (the default: Panda rod outlines for Panda fibers, Mirror symmetry for Bow-tie and Elliptical ones), **Panda rod outlines**, **Mirror symmetry**, **Stress part centres** or **Second moments**, plus **Turn about stress parts** (measure about the centre of the stress parts instead of the fiber centre) and the **Stress part detection** box. On a fiber with lopsided stress parts the methods can differ by a few degrees. **Show stress parts** shades the stress parts the axis was measured from, puts a cross at each one's centre and rings the point the axis turns about, so you can see what the result is based on (also in the exported drawing). How each one works: [Fiber axis formulas](#fiber-axis-formulas) (`camcontrol/processing/fiber_axis.py`, self-test: `python -m camcontrol.processing.fiber_axis`).

**Processing** (Process menu, or select images in Captures and right-click). Results appear in the view marked "(unsaved)"; **File → Save image as** (Ctrl+S) saves them with a `.json` listing the input files and settings. All tools accept any image files, including 3264x1836 HD2 and SD card images, but the images in one run must be the same size (except for stitching).

| Tool | What it does | How to take the images |
|---|---|---|
| Flat-field correction | Evens out uneven lighting and removes fixed dust shadows | Capture an empty, evenly lit field (blank slide), averaging several frames. With it in the view, **Process → Flat-field correction → Use current image as flat reference** (saved to `flats/`). Then turn on **Correct live view and captures** (also a button in the Output box), or correct an opened image. |
| Focus stack | Combines the sharp parts of each image for extended depth of field. Aligns the images first. | Don't move the sample. Step the focus through it, one capture per step. |
| HDR (exposure fusion) | Shows detail in both bright and dark areas | Same view at 3 or more exposures (short, medium, long). |
| Stitch | Joins overlapping images into one large image | Move the sample so neighbouring images overlap by about a third. Needs visible detail in the overlaps. |
| Fluorescence composite | Tints each channel image (one per filter) and adds them together | One capture per filter. Colors are guessed from file names (DAPI, FITC, GFP, TRITC, Cy5, ...) and can be changed. |

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
| `r` | save size: native / HD2-style 1.7x upscale (1920x1080 becomes 3264x1836) |
| `x` / `c` | grid / crosshair |
| `+` / `-` / `0` | zoom in / out / reset (display only) |
| arrows | pan while zoomed |
| `f` | toggle on-screen info |
| `q` / Esc | quit |

## Fiber axis formulas

What the Fiber axis panel calculates, step by step (`camcontrol/processing/fiber_axis.py`). Coordinates are image pixels with $x$ to the right; $y$ is flipped to point up so angles read as on screen. Angles $\theta$ are counter-clockwise from horizontal.

**1. Finding the fiber.** Two finders run, and their circles are pooled:

- Separate fibers: the image is blurred and split into bright and dark with Otsu's threshold. The program keeps outlines that are round ($4\pi A / P^2 > 0.75$, where $A$ is the area and $P$ the perimeter), fill their enclosing circle ($A / \pi r_{enc}^2 > 0.8$), are at least 5% of the image's short side in radius and don't touch the image edge.
- Fibers touching each other or cut off by the image edge (as in a V-groove), found like this:

- The dark gaps and blobs inside the bright regions are filled in, and the deepest point inside them (the distance transform's maximum) gives a first centre and radius for the biggest fiber.
- Its cladding edge is then fitted. Along 360 rays from the centre (0.6R to 1.35R), the edge is the outermost point where the blurred brightness falls below halfway between that ray's cladding level (0.6–0.85R) and the background, moved to the steepest drop within 6% of $R$. Falls less than 30% as steep as the ray's steepest are skipped, so a dim glow outside the fiber isn't taken for its edge, while a shaded, soft side still counts. A least-squares circle $x^2 + y^2 = 2ax + 2by + c$ is fitted to those points, refitting 5 times without points more than $2.5 \times$ their robust spread off it (where a neighbour touches, dirt). The whole thing is repeated 3 times from the new circle.
- The other fibers are taken to be the same size. Each strong edge votes for the point one radius $R$ away on its bright side (a Hough transform with the radius fixed). Fibers are the peaks of at least 12% of the strongest, at least $1.5R$ apart, and may be centred off the image.

Every circle is then fitted to its cladding edge as above, so dirt on the edge or a bright halo round it doesn't shift it, and circles that repeat one already kept (centres closer than 0.8 of the radius: the same fiber, or a rod or dark patch inside it) are dropped. If nothing is found, circle detection on edges (Hough) is used. A Circle measurement replaces this step when **Use circle measurements** is on. **Only the centre fiber** keeps just the fiber found nearest the middle of the image.

**2. Finding the stress parts.** Only a ring of the fiber is used: $0.12R < d < 0.92R$, where $d$ is the distance from the fiber centre. That leaves out the core and the cladding edge. The image is converted to Lab color and each channel $k$ (L brightness, a and b color) is blurred (σ = smoothing × $R$, default 1%). For each channel a smooth surface is fitted to the cladding only:

$$S_k(u, v) = c_0 + c_1 u + c_2 v + c_3 (u^2 + v^2), \quad u = x/R,\ v = y/R$$

a tilt plus a round bowl, for uneven lighting and vignetting. It has no axis of its own, so it can't fit away a pair of stress parts. The fit starts from the pixels near the channel's commonest value (the cladding covers most of the ring), then is refitted 5 times leaving out pixels more than $2\sigma_k$ off it, where $\sigma_k = 1.4826 \cdot \mathrm{median}|\mathrm{off}|$ (+0.5) is the cladding's noise. The contrast of each pixel, in noise units, is

$$D = \sqrt{\sum_k \left(\frac{\mathrm{Lab}_k - S_k}{\sigma_k}\right)^2}$$

over the channels chosen in **Look at** (brightness and color, brightness only, or color only). Dividing by the noise means a faint brightness-only difference counts as much as a strong color one, and a channel full of texture (e.g. moiré) counts less. The stress-part mask is the ring pixels with $D > T$, where $T$ is Otsu's threshold of $D$ (at least 2.5) or the value set with the **Threshold** slider. Regions smaller than a square of side "Ignore specks" × $R$ (default 8%) are dropped.

**Low contrast.** If the shaded stress parts don't match the image, use the **Stress part detection** box: untick **Auto** and lower **Threshold** to catch fainter parts (the slider starts at the automatic value), raise **Smoothing** for noisy images, try **Brightness only** when color fringes or dirt are being picked up, and raise **Ignore specks** to drop small stray regions. With **Show stress parts** and **Update live** on, the shading follows the sliders. The settings are remembered, and **Defaults** puts them back. Mirror symmetry uses $D$ itself, not the mask, so the threshold only changes the other two methods, the type guess and the Clarity.

**3. The slow axis.** By the **Method** box:

- **Automatic** (default). Panda rod outlines (below), unless the fiber looks like a Bow-tie or Elliptical one; then Mirror symmetry, with that type. To decide, three outlines are fitted to the same edge map, each scored like the rods (the mean capped gradient across the outline, over the cap, 0–1): the two rod circles found; the best pair of wedges (annular sectors either side of the core: inner radius $0.03$–$0.42R$, outer $0.35$–$0.9R$, half-angle 8–62°); and the best ellipse round the core (half-axes $0.25$–$0.85R$ and $0.12$–$0.6R$, long axis along or across the rods' axis). Both are searched within ±6° of the rods' axis. The wedges or ellipse win only if the rod search found strong edges (quality ≥ 2.4: wedges and ellipses give 2.7 and more, a failed search on a dirty Panda about 2), the two parts point opposite ways (rod 0's outline matches rod 1 at least 0.05 better turned 180° than side by side, see step 4 below; Panda rods match side by side) and they score higher than the rods. It leans to Panda on purpose: a Bow-tie measured by its "rods" is off by about a degree, but a dirty Panda measured by mirror symmetry can be off by ten.

- **Panda rod outlines.** Skips step 2 and works on edges only, so shading, dark patches and most dirt hardly count. The fiber is scaled so its radius is 200 px, blurred (σ = 1.5 px), and the brightness gradient $\mathbf{g}$ is taken. Gradients beyond $0.94R$ (cladding edge) or off the image are left out. *Dirt* is every pixel more than 6 noise levels brighter or darker than the median of the 31 × 31 px round it (rods are far bigger than that, so only specks, streaks and thin lines stand out), widened by 3 px; the noise level is $1.4826 \cdot \mathrm{median}$ of that difference (at least 2). A circle's score is the mean, round it, of $\min(\lvert \mathbf{g} \cdot \mathbf{n} \rvert,\ 4\tau)$, where $\mathbf{n}$ is the circle's outward normal and $\tau$ the median gradient inside the fiber (mostly noise; at least 0.5). The cap means a circle scores by how much of it lies on an edge, not by how strong a few edges are, so the sharp edges of dust and bubbles can't pull it. A thin bright or dark outline counts as much as a step.

  1. Every pair of equal circles either side of a middle point is tried: the middle within $\pm 0.08R$ of the fiber centre in x and y, in $0.02R$ steps (the core can sit a little off the fitted cladding centre), angle $\theta$ in 2° steps, distance $s$ of each centre from the middle $0.30$–$0.72R$, radius $\rho$ $0.18$–$0.42R$ (Panda rods are about $0.25$–$0.3R$, so dust-sized circles aren't tried), both inside $0.92R$. The pair's score is its weaker circle's, so both rods must be there. This is done twice: with dirt left out, so a cluster of specks can't crowd out faint rods, and as is, since a drawn outline or the dark rim of an embossed rod looks just like dirt.
  2. The 5 best different pairs of each search, and the best centred on the fiber, are refined as one shape: the middle point (the core), $\theta$, $s$ and $\rho$, in steps from 2 px (1°) down. Panda rods are symmetric about the core, so dirt over one rod can't drag it away on its own. The pair kept has the highest (mean of its score with and without dirt) × (outline found, see below): it must follow edges all the way round both rods, not just cross a few strong ones.
  3. Each rod is then fitted to a fraction of a pixel: on 180 rays from its centre, the strongest edge within 12% of $\rho$; a least-squares circle through those points, leaving out stray ones (dust); repeated 3 times. The two rods' radii are averaged.
  4. A rod whose own outline hardly shows (under 50% found) is put opposite the other one, through the point between them. Otherwise the edge strength in a ring round rod 0 ($0.75$–$1.25\rho$, without dirt and the core) is slid over rod 1 (normalized cross-correlation, peak to a fraction of a pixel). Both rods are lit the same way, so even lopsided, embossed outlines match, and the shift between them gives the rod-to-rod line more exactly than the edge fits, which dirt next to a rod can pull. It is used if the match scores at least 0.75 and its angle is within 3° of step 3's.

  The slow axis is the line through the two rod centres, drawn through the point between them. Two numbers say how clear it is. *Quality* is the best pair's score over the median score of all pairs tried; at least 2.0. *Outline found* (shown in Clarity) is checked on 32 arcs of each rod: the share of arcs where the strongest edge within 5% of $\rho$ is clear (over $2\tau$) and at least 60% of the strongest within 25%. The smaller of the two rods' values is used; at least 50% (clean rods give 80–100%, dirty ones 50–80%). Below either limit the result is marked "(unclear)" and the type is "?".

  Tested on synthetic images with known axes (`camcontrol` doesn't ship them), 80 of each kind. *Mixed*: Panda fibers with faint outlines, shading patches over the rods, dust, bubbles, dirt streaks and debris, dirt on the cladding edge, noise, blur, moiré, and touching neighbours cut off by the image edge: the angle was within 1° on 94% (median 0.18°, worst 1.3°). *Relief*: obliquely lit, embossed rods, one side of the cladding fading into shade, a glow round the rim, a thick black streak along a rod and clusters of debris: within 1° on 94% (median 0.09°, worst 3.5°). It takes about 0.25 s per fiber (finding it about 0.07 s, the rods the rest; the two searches of step 1 run side by side).

- **Second moments.** For the $N$ mask pixels at offsets $(x_i, y_i)$ from the pivot point:

  $$\mu_{20} = \tfrac{1}{N}\textstyle\sum x_i^2, \quad \mu_{02} = \tfrac{1}{N}\sum y_i^2, \quad \mu_{11} = \tfrac{1}{N}\sum x_i y_i$$

  $$\theta = \tfrac{1}{2}\,\mathrm{atan2}\!\left(2\mu_{11},\ \mu_{20} - \mu_{02}\right)$$

  This is the long axis of the ellipse that best fits the mask. Each pixel counts by its distance squared, so the parts far from the centre (the outer corners of bow-tie wedges) decide the angle most.

- **Stress part centres.** Small specks are removed (morphological opening, then parts smaller than $(0.08R)^2$ are dropped). Then the angle is that of the line through the centres $(a_x, a_y)$ and $(b_x, b_y)$ of the two largest parts:

  $$\theta = \mathrm{atan2}\!\left(-(b_y - a_y),\ b_x - a_x\right)$$

  Each part counts the same whatever its shape or size. With only one part (Elliptical-clad), Second moments is used instead, and the table's Method column says so.

- **Mirror symmetry**. PM fibers are mirror-symmetric about both the slow and fast axes. The difference map $D$ inside the ring is scaled so the fiber's radius is about 200 px. For each candidate angle $\theta$, the map is turned by $-\theta$ about the pivot, so the candidate axis becomes horizontal. It is then compared with its own mirror images:

  $$E(\theta) = \mathrm{mean}_{ring}\left(\,\lvert M - M_{\updownarrow} \rvert + \lvert M - M_{\leftrightarrow} \rvert\,\right)$$

  Here $M$ is the turned map, $M_{\updownarrow}$ is $M$ flipped top to bottom (mirror about the candidate slow axis) and $M_{\leftrightarrow}$ is $M$ flipped left to right (mirror about the fast axis). The angle with the smallest $E$ wins. The search is ±20° around the Second moments angle in 1° steps, then ±1° in 0.05° steps. No threshold is involved, and one lopsided stress part pulls the result less than in the other methods. It takes about 1.5 times as long as the others.

**Pivot.** Second moments and Mirror symmetry turn about the fiber centre $(c_x, c_y)$ by default. With **Turn about stress parts** on, they turn about the mask's centre $(\bar{x}, \bar{y})$ instead, and the offsets become $(x_i - \bar{x},\ y_i - \bar{y})$. Stress part centres always goes through the midpoint of the two centres. The axis lines are drawn through the point used, shown in the "Axis through" column.

**4. Fast axis, signed angle and clarity.** The fast axis is $\theta + 90°$. The table shows angles from −90° to +90°: $\theta_{signed} = ((\theta + 90) \bmod 180) - 90$. **Clarity** for Panda rod outlines is the outline found (step 3). For the other methods it comes from the second moments. The spreads along and across the axis are the eigenvalues

$$\lambda_{1,2} = \frac{\mu_{20} + \mu_{02}}{2} \pm \sqrt{\left(\frac{\mu_{20} - \mu_{02}}{2}\right)^2 + \mu_{11}^2}$$

and clarity $= \sqrt{\lambda_1 / \lambda_2}$, the length-to-width ratio of the fitted ellipse. A value of 1 means round, so there's no axis; below 1.3 the result is marked "(unclear)".

**5. Type guess.** One stress part means Elliptical. Two parts are Panda if both are round ($4\pi A / P^2 > 0.8$), otherwise Bow-tie. Any other count gives "?".

**Example** (bow-tie images `36879B-00FBB-001.tif` and `-008.tif`, whose two wedges are slightly lopsided): Second moments −2.7°, Stress part centres −1.4°, Mirror symmetry +0.5°. All three follow a known rotation of the image to within 0.01°. They differ only in what they treat as "the axis" of an imperfect fiber. Automatic measures both by mirror symmetry, as Bow-tie: the wedges fit 0.16 and 0.12 better than circles, and the two parts match better turned 180° than side by side.

## Hardware

- **Camera:** O.C. White 6MP Ultra-Cam II (SKU 14305), hybrid HDMI/USB
- **Optics:** O.C. White MacroZoom lens (or trinocular microscope)
- **Connection:** USB to a Windows PC. The camera appears as a camera device in Windows, so it should be readable with OpenCV and no vendor SDK.

Known camera facts (from the O.C. White MacroZoom spec sheet, 2021):

| Spec | Value |
|---|---|
| Live preview | 1920 x 1080 (2MP) |
| Still capture (onboard, to SD card) | 3264 x 1836 ("6MP"); really 1080p enlarged in the camera, see below |
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

**1920 x 1080 is the largest size available over USB, and it is the camera's real resolution.** The "6MP" SD-card photos are 1080p enlarged too (checked 2026-10-01 on `IMG00000-2.JPG`): they are 3264 x 1840 JPEGs (height rounded up to a multiple of 16) with a strong resampling peak at 0.41 cycles/pixel in both directions, the fingerprint of a ~1.7x stretch from about 1920 x 1080 (across 0.4125, down 0.4138). The camera's own scaler uses a ratio slightly off 1.7, so the pattern drifts rather than repeating exactly every 17 pixels. The camera then sharpens and JPEG-compresses them, and burns in its scale-bar overlay. A native 1920 x 1080 capture over USB (TIFF/PNG) holds all the real detail there is.

**Frames.** Frames come through OpenCV with DirectShow (`cv2.CAP_DSHOW`). Set the frame size **before** the MJPG FOURCC. In the other order, DirectShow silently uses YUY2 at 5.7 fps. With the right order, 1080p runs at about 25 fps. Media Foundation gives 30 fps but can't set controls reliably.

**Controls** are set through `camcontrol/uvc_controls.py`, which calls the Windows camera interfaces (`IAMCameraControl`, `IAMVideoProcAmp`) directly via `comtypes`, not through OpenCV. OpenCV caps gain at 32, and it can't get exposure working again after HD2 has used the camera. Run `python -m camcontrol.uvc_controls` to list every control, its range and its current value.

| Control | Range | Works? | Notes |
|---|---|---|---|
| Exposure | -13 to -3 | Yes | **Real exposure ≈ 10 × 2^value s** (about 1.2 ms to 1.25 s), not the nominal 2^value. Measured from frame timing. The app's slider goes on to 1 (≈ 20 s) by adding frames together in software, as HD2 does: -2 ≈ 2.5 s (2 frames), -1 ≈ 5 s, 0 ≈ 10 s, 1 ≈ 20 s (16 frames). Live view shows a running sum, so it still updates every 1.25 s; captures add fresh frames, and their `.json` records `frames_summed_per_exposure`. |
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
- **How the upscale is done (checked 2026-10-01 in HD2's program files).** The size list in `TSCamPluginsMgr.dll` has `3264x1836` hard-coded as the save size for this camera. Saving goes through `ImageConvert::ResizeTSImg`, which calls OpenCV's ordinary `cv::resize` (HD2 ships OpenCV 2.4.10: `opencv_imgproc2410.dll`). `RS_RESOLUTIONINTER=2` is the interpolation setting; as an OpenCV flag, 2 is `INTER_CUBIC` (bicubic). Two HD2 TIFs (`36879B-00FBB-001/008.tif`) carry the resampling fingerprint: the pixel-to-pixel differences repeat every 17 pixels (10 source pixels become 17) in both directions, about 8 times stronger than at any other period. Their pattern is closest to bicubic, but no plain OpenCV method reproduces it exactly, so HD2 probably also processes the image (its camera profile has software Sharpness 3 and Denoise 5). The TIFs' strongest pattern repeats every 13.6 pixels, which is the camera's 8 x 8 MJPEG blocks stretched 1.7x: more evidence that the source is the 1080p USB frame.
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
6. Image processing tools, usable on live captures or saved files (including the camera's 3264x1836 SD card images):
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
│   ├── image_io.py      # reading image files (handles non-ASCII paths, 16-bit)
│   ├── camera.py        # open camera, set resolution/controls, grab frames
│   ├── uvc_controls.py  # direct DirectShow camera controls (comtypes)
│   ├── capture.py       # frame averaging, save image + JSON metadata
│   ├── recording.py     # video recording and time-lapse (run by the camera thread)
│   ├── settings_file.py # settings files (JSON): save, load, recent list
│   ├── overlays.py      # display-only zoom/pan, grid, crosshair
│   ├── viewer.py        # simple OpenCV live view (no Qt)
│   ├── gui/
│   │   ├── main_window.py   # window, controls panel, menus, status bar
│   │   ├── image_view.py    # zoom/pan image view, grid, crosshair, measurement tools
│   │   ├── measure_panel.py # tool buttons, results table, delete/clear/export
│   │   ├── measure_draw.py  # draws measurements (on screen and for the annotated export)
│   │   ├── qt_image.py      # numpy -> QImage
│   │   ├── gallery.py       # Captures panel: recent-image thumbnails
│   │   ├── count_panel.py   # Counting panel, drawing marks
│   │   ├── process_dialogs.py # dialogs for focus stack, HDR, stitch, fluorescence
│   │   ├── jobs.py          # runs processing in the background
│   │   ├── compare.py       # compare grid
│   │   ├── record_panel.py  # Video and Time-lapse tabs of the Controls panel
│   │   ├── fiber_panel.py   # Fiber axis panel and drawing
│   │   ├── video_player.py  # video playback bar
│   │   ├── tool_stripes.py  # PyCharm-style panel stripes along the window edges
│   │   └── camera_worker.py # camera thread (keeps the GUI responsive)
│   ├── calibration.py   # per-axis µm/px (only "pixels" so far; JSON table in Phase 2)
│   ├── measure.py       # measurement geometry and results (self-test: python -m camcontrol.measure)
│   ├── export.py        # CSV / Excel output
│   └── processing/      # no GUI code; each runs a self-test: python -m camcontrol.processing.hdr
│       ├── common.py        # alignment, color helpers
│       ├── flatfield.py
│       ├── focus_stack.py
│       ├── hdr.py
│       ├── stitch.py
│       ├── count.py         # counting marks and export
│       ├── fiber_axis.py    # PM fiber slow/fast axis
│       └── fluorescence.py
├── tools/
│   ├── probe_camera.py  # Phase 0 diagnostic
│   └── camera_settings.py  # opens the driver's own settings dialog
├── calibrations/
│   └── calibrations.json
├── flats/               # flat-field references taken in the app
├── settings/            # settings files (default location)
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

### Phase 4: Processing tools (done)
- Flat-field correction (divide by a blank reference frame).
- Focus stacking (align frames, pick sharpest pixels by Laplacian).
- HDR via `cv2.createMergeMertens`.
- Stitching via `cv2.Stitcher_create(cv2.Stitcher_SCANS)`.
- Class counting and fluorescence composites.
- Possible later additions: a dark-frame option for flat-field, showing the focus-stack depth map, a live preview while adjusting fluorescence colors.

### Phase 5: GUI (shell done; built ahead of Phases 2–4)
- Done: PySide6 app with live view, camera controls, capture, overlays, freeze and open-file.
- Measurements are drawn in `ImageView.drawForeground` from the panel's list, not as QGraphicsItems. That's simpler, but shapes can't be dragged to edit them; delete and redraw instead.
- To do: add calibration once Phase 2 is built.

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
- Processing tools should accept file paths as input, not only live frames, so HD2 and SD card images can be used.
- Target Python 3.14+ and use PySide6 (not PyQt) for all GUI code.
- Prefer clear, commented code over clever code. The user is building this to learn and to own it.

