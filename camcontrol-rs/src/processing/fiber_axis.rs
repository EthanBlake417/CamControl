//! Axis of symmetry of polarization-maintaining (PM) fiber end faces.
//! A port of camcontrol/processing/fiber_axis.py; see there for the full story.
//!
//! PM fibers have stress parts on either side of the core: two round rods
//! (Panda), two wedges (Bow-tie), or an elliptical region round the core
//! (Elliptical-clad). The line through them is the slow axis; the fast axis
//! is at 90 degrees.
//!
//! For each fiber: find the cladding circle, then the slow axis by one of
//! METHODS ("auto" picks rods for Panda fibers, mirror symmetry for Bow-tie
//! and Elliptical ones, and the apple core when the rods are washed out). Angles are in degrees, counter-clockwise from
//! horizontal as seen on screen, 0 to 180.

use std::f64::consts::PI;
use std::sync::OnceLock;

use image::RgbImage;
use serde::{Deserialize, Serialize};

use crate::imgproc::{self as ip, ImgF, Mask};
use super::fiber_quality::{fiber_quality, rod_contrast, rod_edge_width, FiberQuality};

pub const CORE_FRACTION: f64 = 0.12;
pub const EDGE_FRACTION: f64 = 0.92;
pub const MIN_ELONGATION: f64 = 1.3;
const SYMMETRY_SIZE: f64 = 200.0;
const NOISE_FLOOR: f64 = 0.5;
const MIN_AUTO_THRESHOLD: f64 = 2.5;
const FALL_SHARE: f32 = 0.3;
const FIT_SAMPLES: f64 = 40000.0;
const ROD_SEARCH_EDGE: f64 = 0.94;
const ROD_MAX_REACH: f64 = 0.92;
const ROD_SIZES: (f64, f64) = (0.18, 0.42);
pub const DEFAULT_MIN_ROD: f64 = 18.0; // % of the radius: ROD_SIZES.0, i.e. every size searched
pub const MAX_MIN_ROD: f64 = 40.0;
const ROD_SPACINGS: (f64, f64) = (0.30, 0.72);
const EDGE_CAP: f32 = 4.0;
const GRADIENT_FLOOR: f32 = 0.5;
const DIRT_CONTRAST: f32 = 6.0;
const DIRT_WINDOW: usize = 31;
const ROD_STARTS: usize = 5;
const MATCH_SCORE: f32 = 0.75;
const MATCH_AGREE: f64 = 3.0;
pub const MIN_ROD_QUALITY: f64 = 2.0;
const ON_CIRCLE_SHARE: f32 = 0.6;
pub const MIN_ROD_COVERAGE: f64 = 0.5;
const OTHER_SHAPE_MARGIN: f64 = 0.0;
const TURNED_GAIN: f64 = 0.05;
const OTHER_SHAPE_QUALITY: f64 = 2.4;
const MIN_SHOWN: f32 = 0.5;         // rod outlines off the image: scored over the part shown, at least this share
const CORE_SEARCH: f64 = 0.15;      // a visible core spot is looked for within this fraction of the radius
const CORE_SIGNIFICANCE: f32 = 8.0; // ... standing out this many noise levels from the cladding
const CORE_SPREAD: f64 = 0.08;      // rod pairs centered further from it than this (fraction of the radius) count less
const CORE_TIE: f32 = 0.9;          // ... among the pairs ranked at least this share of the best
const APPLE_REACH: f64 = 0.3;       // the apple core is looked at within this fraction of the radius
const APPLE_SIZE: f64 = 100.0;      // ... on a copy this many pixels across that reach
const APPLE_SYMMETRY: (f64, f64) = (0.8, 0.3); // mirror correlation needed about its better and its other axis
const APPLE_PINCH: f64 = 0.05;      // the pinched sides' corners reaching this much (share of the middle) further than the others'
const APPLE_SIDE: f64 = 75.0;       // degrees either side of an axis to trace the outline
const APPLE_AGREE: f64 = 3.0;       // degrees: rods and apple core agreeing
const APPLE_ROD_GUIDE: f64 = 10.0;  // degrees: rod outlines this close to one of the core's axes pick it as the slow one
const APPLE_ROD_QUALITY: f64 = 1.8; // ... if they stand out at least this much (washed out is fine, nothing at all isn't)

/// name, label in the panel
pub const METHODS: [(&str, &str); 6] = [
    ("auto", "Automatic"),
    ("rods", "Panda rod outlines"),
    ("core", "Apple core"),
    ("symmetry", "Mirror symmetry"),
    ("centres", "Stress part centers"),
    ("moments", "Second moments"),
];

/// name, label, Lab channels used
pub const LOOK_AT: [(&str, &str, &[usize]); 3] = [
    ("both", "Brightness and color", &[0, 1, 2]),
    ("brightness", "Brightness only", &[0]),
    ("color", "Color only", &[1, 2]),
];

pub fn method_label(name: &str) -> &'static str {
    METHODS.iter().find(|m| m.0 == name).map(|m| m.1).unwrap_or("?")
}

/// How stress parts are told apart from the cladding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Detection {
    /// Contrast (noise units) a stress part must reach; None: automatic.
    pub threshold: Option<f64>,
    /// Blur, % of the fiber radius.
    pub smoothing: f64,
    /// See LOOK_AT.
    pub look_at: String,
    /// Smaller regions are specks: side of a square, % of the radius.
    pub min_size: f64,
    /// Panda rod outlines: smallest rod radius considered, % of the fiber radius
    /// (smaller round things, such as dust or the core, are never taken for a rod).
    /// Used when min_rod_px is 0.
    pub min_rod: f64,
    /// The same in image pixels (0: not set), as drawn on the view; each fiber
    /// gets it as its own share of its radius.
    pub min_rod_px: f64,
}

impl Default for Detection {
    fn default() -> Self {
        Self { threshold: None, smoothing: 1.0, look_at: "both".into(), min_size: 8.0, min_rod: DEFAULT_MIN_ROD, min_rod_px: 0.0 }
    }
}

impl Detection {
    /// Smallest rod radius for a fiber of this radius (pixels), % of the radius.
    pub fn min_rod_for(&self, radius: f64) -> f64 {
        if self.min_rod_px > 0.0 && radius > 0.0 {
            (100.0 * self.min_rod_px / radius).clamp(DEFAULT_MIN_ROD, MAX_MIN_ROD)
        } else {
            self.min_rod
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FiberAxis {
    pub cx: f64,
    pub cy: f64,
    pub radius: f64,
    pub slow_deg: f64,
    pub elongation: f64,
    pub fiber_type: String,
    pub n_parts: usize,
    pub method: String,
    pub axis_x: Option<f64>,
    pub axis_y: Option<f64>,
    pub parts: Vec<Vec<(f64, f64)>>,
    pub part_centres: Vec<(f64, f64)>,
    pub threshold: f64,
    pub quality: Option<f64>,
    pub coverage: Option<f64>,
    pub rod_radii: Vec<f64>,
    pub turned_gain: Option<f64>,
    /// Brightness (0-255) of the brightest spot near the center (light in the core), see centre_brightness.
    pub centre_brightness: Option<f64>,
    /// Measured from the apple core (see apple_core): whether it was clear.
    pub core_clear: Option<bool>,
    /// Contrast, focus, noise and clipping round the fiber, see fiber_quality.
    pub image_quality: Option<FiberQuality>,
    /// Focus on the stress rods: their edges' width in px (smaller is sharper), see fiber_quality::rod_edge_width.
    pub rod_edge: Option<f64>,
    /// How clearly the stress rods stand out from the cladding round them (-1 to 1), see fiber_quality::rod_contrast.
    pub rod_contrast: Option<f64>,
}

impl FiberAxis {
    pub fn new(cx: f64, cy: f64, radius: f64, slow_deg: f64, elongation: f64, fiber_type: &str, n_parts: usize) -> Self {
        Self {
            cx,
            cy,
            radius,
            slow_deg,
            elongation,
            fiber_type: fiber_type.into(),
            n_parts,
            method: "moments".into(),
            axis_x: None,
            axis_y: None,
            parts: Vec::new(),
            part_centres: Vec::new(),
            threshold: 0.0,
            quality: None,
            coverage: None,
            rod_radii: Vec::new(),
            turned_gain: None,
            centre_brightness: None,
            core_clear: None,
            image_quality: None,
            rod_edge: None,
            rod_contrast: None,
        }
    }

    pub fn axis_point(&self) -> (f64, f64) {
        match (self.axis_x, self.axis_y) {
            (Some(x), Some(y)) => (x, y),
            _ => (self.cx, self.cy),
        }
    }

    pub fn fast_deg(&self) -> f64 {
        (self.slow_deg + 90.0).rem_euclid(180.0)
    }

    pub fn clear(&self) -> bool {
        if let Some(c) = self.core_clear {
            return c;
        }
        if let Some(q) = self.quality {
            return q >= MIN_ROD_QUALITY && self.coverage.unwrap_or(0.0) >= MIN_ROD_COVERAGE;
        }
        self.elongation >= MIN_ELONGATION
    }
}

/// numpy's / Python's round: halves to even.
#[inline]
fn rne(x: f64) -> f64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 { 2.0 * (x / 2.0).round() } else { r }
}

/// np.arange(start, stop, step) as f64 values.
fn arange(start: f64, stop: f64, step: f64) -> Vec<f64> {
    let n = ((stop - start) / step).ceil().max(0.0) as usize;
    (0..n).map(|i| start + i as f64 * step).collect()
}

/// np.linspace(a, b, n, endpoint)
fn linspace(a: f64, b: f64, n: usize, endpoint: bool) -> Vec<f64> {
    let d = if endpoint { (n.max(2) - 1) as f64 } else { n as f64 };
    (0..n).map(|i| a + (b - a) * i as f64 / d).collect()
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len().max(1) as f64
}

// --- finding fibers ------------------------------------------------------------------

/// Round fibers in the image as (cx, cy, radius), the biggest first.
pub fn find_fibers(image: &RgbImage, max_fibers: usize) -> Vec<(f64, f64, f64)> {
    let gray = ip::to_gray(image);
    let mut found: Vec<(f64, f64, f64)> = Vec::new();
    let bright = otsu_mask(&gray);
    let touching = touching_fibers(&gray, &bright, max_fibers, FIRST_MIN_R);
    let had_touching = !touching.is_empty();
    let candidates: Vec<_> = touching.into_iter().chain(round_regions(&gray, &bright, FIRST_MIN_R)).collect();
    // Every candidate's edge fit at once (some are dropped below as repeats).
    let fits: Vec<Option<(f64, f64, f64)>> = std::thread::scope(|s| {
        let jobs: Vec<_> = candidates
            .iter()
            .enumerate()
            .map(|(i, &c)| {
                let gray = &gray;
                s.spawn(move || if i == 0 && had_touching { Some(c) } else { fit_cladding_edge(gray, c.0, c.1, c.2, 360) })
            })
            .collect();
        jobs.into_iter().map(|j| j.join().unwrap()).collect()
    });
    for (i, mut c) in candidates.into_iter().enumerate() {
        if found.iter().any(|k| (c.0 - k.0).hypot(c.1 - k.1) < 0.5 * k.2.max(c.2)) {
            continue; // the same fiber again
        }
        let fitted = fits[i];
        if let Some(f) = fitted {
            if (f.2 - c.2).abs() < 0.2 * c.2 && (f.0 - c.0).hypot(f.1 - c.1) < 0.2 * c.2 {
                c = f;
            }
        }
        if found.iter().all(|k| (c.0 - k.0).hypot(c.1 - k.1) > 0.8 * k.2.max(c.2)) {
            found.push(c);
        }
    }
    if found.is_empty() {
        found = find_hough_circles(&gray, 10);
    }
    found = faint_cladding_around(&gray, found);
    // No edge all round (a step or an outline): not a fiber (the curved edge of the lit
    // area, shading); then fibers this search missed.
    let check = EdgeCheck::new(&gray);
    found.retain(|c| check.has_edge(c));
    found = more_fibers(&gray, &check, found, max_fibers);
    found = prune_nested(found);
    found.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    found.truncate(max_fibers);
    found
}

/// Fibers the first search missed, each kept only with a clear cladding edge
/// (EdgeCheck::is_fiber) and not overlapping one found: from the bright and dark split
/// of the image with its lighting evened out (a slope of light across the image splits
/// the image instead of the fibers), from circles by their edges' shape (no split at
/// all), and circles the size of the best-edged fiber found (fibers side by side in a
/// groove are the same size).
fn more_fibers(gray: &image::GrayImage, check: &EdgeCheck, mut found: Vec<(f64, f64, f64)>, max_fibers: usize) -> Vec<(f64, f64, f64)> {
    // All on the image with its lighting evened out: the edge fits take the background as
    // one level, which a slope of light across the image breaks.
    let flat = flatten_lighting(gray);
    let gray = &flat;
    let bright = otsu_mask(gray);
    // Smaller fibers too than the first search looks for (many in a groove, each small in the picture).
    let similar = |a: &(f64, f64, f64), b: &(f64, f64, f64)| a.2 < 1.5 * b.2 && b.2 < 1.5 * a.2 && (a.0 - b.0).hypot(a.1 - b.1) < 0.8 * a.2.max(b.2);
    let rough: Vec<(f64, f64, f64)> = touching_fibers(gray, &bright, max_fibers, MORE_MIN_R).into_iter()
        .chain(round_regions(gray, &bright, MORE_MIN_R))
        .chain(hough_circles_sized(gray, max_fibers, 1000.0, None, MORE_MIN_R, 2.0 * MORE_MIN_R))
        .filter(|c| !found.iter().any(|k| similar(c, k)))
        .collect();
    let fits: Vec<Option<(f64, f64, f64)>> = std::thread::scope(|s| {
        let jobs: Vec<_> = rough.iter().map(|&c| s.spawn(move || fit_cladding_edge(gray, c.0, c.1, c.2, 360))).collect();
        jobs.into_iter().map(|j| j.join().unwrap()).collect()
    });
    // Fitted (where the fit agrees) and checked, biggest first: of overlapping ones (a fiber
    // and a stress rod or core inside it) the fiber wins.
    let mut extra: Vec<(f64, f64, f64)> = rough.iter().zip(fits).filter_map(|(c, f)| match f {
        Some(f) if (f.2 - c.2).abs() < 0.2 * c.2 && (f.0 - c.0).hypot(f.1 - c.1) < 0.2 * c.2 => Some(f),
        _ => None,
    }).filter(|c| check.is_fiber(c)).collect();
    extra.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    for c in extra {
        if found.len() < max_fibers && !found.iter().any(|k| similar(&c, k)) {
            found.push(c);
        }
    }
    same_size_neighbours(gray, check, prune_nested(found), max_fibers)
}

/// Smallest fiber radius looked for, share of the image's short side: by the first search,
/// and by the extra searches (a groove full of fibers, each small in the picture).
const FIRST_MIN_R: f64 = 0.05;
const MORE_MIN_R: f64 = 0.02;

/// Fibers don't lie in each other: a circle round several found fibers is something else
/// (the edge of the lit area, a column of fibers); a circle inside a bigger fiber is one of its
/// stress rods or its core.
fn prune_nested(found: Vec<(f64, f64, f64)>) -> Vec<(f64, f64, f64)> {
    let inside = |small: &(f64, f64, f64), big: &(f64, f64, f64)| small.2 < 0.7 * big.2 && (small.0 - big.0).hypot(small.1 - big.1) < big.2;
    // Much smaller fibers in it or across its edge: three or more and it isn't a fiber.
    let overlapping = |small: &(f64, f64, f64), big: &(f64, f64, f64)| small.2 < 0.5 * big.2 && (small.0 - big.0).hypot(small.1 - big.1) < big.2 + small.2;
    let kept: Vec<_> = found.iter().filter(|b| found.iter().filter(|s| overlapping(s, b)).count() < 3).copied().collect();
    kept.iter().filter(|s| !kept.iter().any(|b| inside(s, b))).copied().collect()
}

/// The gray image with its lighting evened out: a smooth surface (quadratic in
/// x and y) fitted to a small copy, leaving out what stands out from it (fibers),
/// subtracted, around mid-gray.
pub fn flatten_lighting(gray: &image::GrayImage) -> image::GrayImage {
    let (w, h) = (gray.width() as usize, gray.height() as usize);
    let scale = (160.0 / w.max(h) as f64).min(1.0);
    let (sw, sh) = ((w as f64 * scale).round().max(2.0) as usize, (h as f64 * scale).round().max(2.0) as usize);
    let small = ip::resize_area(&ImgF::from_gray(gray), sw, sh);
    let terms = |u: f64, v: f64| vec![1.0, u, v, u * u, u * v, v * v];
    let pts: Vec<(f64, f64, f64)> = (0..sh).flat_map(|y| (0..sw).map(move |x| (x, y)))
        .map(|(x, y)| (2.0 * (x as f64 + 0.5) / sw as f64 - 1.0, 2.0 * (y as f64 + 0.5) / sh as f64 - 1.0, small.at(x, y) as f64))
        .collect();
    let mut keep = vec![true; pts.len()];
    let mut coef = vec![0.0; 6];
    for _ in 0..4 {
        let (rows, b): (Vec<Vec<f64>>, Vec<f64>) = pts.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| (terms(p.0, p.1), p.2)).unzip();
        if rows.len() < 12 {
            return gray.clone();
        }
        let Some(c) = ip::lstsq(&rows, &b) else { return gray.clone() };
        coef = c;
        let res: Vec<f64> = pts.iter().map(|p| p.2 - terms(p.0, p.1).iter().zip(&coef).map(|(t, c)| t * c).sum::<f64>()).collect();
        let spread = 1.4826 * ip::median_f64(&res.iter().map(|r| r.abs()).collect::<Vec<_>>()) + 1.0;
        for (k, r) in keep.iter_mut().zip(&res) {
            *k = r.abs() < 2.5 * spread;
        }
    }
    let level = |x: usize, y: usize| {
        let (u, v) = (2.0 * (x as f64 + 0.5) / w as f64 - 1.0, 2.0 * (y as f64 + 0.5) / h as f64 - 1.0);
        terms(u, v).iter().zip(&coef).map(|(t, c)| t * c).sum::<f64>()
    };
    image::GrayImage::from_fn(w as u32, h as u32, |x, y| {
        let v = gray.get_pixel(x, y).0[0] as f64 - level(x as usize, y as usize) + 128.0;
        image::Luma([v.round().clamp(0.0, 255.0) as u8])
    })
}

/// A found circle is kept as a fiber only if it's mostly on the image and has a
/// cladding edge round most of it: brighter (or darker) just inside than just
/// outside, the same way, on at least EDGE_SUPPORT of the rays, and changing
/// there much more than just inside or outside (so not a slope of light across
/// the image). Curved edges of the lit area, shading and dirt don't have that.
const EDGE_RAYS: usize = 72;
const EDGE_SUPPORT: f64 = 0.55; // rays with a step the same way
const SHARP_SUPPORT: f64 = 0.3; // ... and rays where it's an edge, not a slope of light (see support)
const SLOPE_MARGIN: f32 = 0.8;
const OUTLINE_SUPPORT: f64 = 0.7; // rays with a step or an outline (first search: see has_edge)
const ON_IMAGE: f64 = 0.4; // share of the rays that must be on the image (fibers in a groove run off it)
const MIN_STEP: f32 = 3.0; // gray levels, at least (compressed video shows no noise)

pub struct EdgeCheck {
    img: ImgF,
    noise: f32,
}

impl EdgeCheck {
    pub fn new(gray: &image::GrayImage) -> Self {
        let img = ImgF::from_gray(gray);
        // Pixel noise from neighbours' differences, every few rows.
        let mut d = Vec::new();
        for y in (0..img.h).step_by(4) {
            for x in (0..img.w.saturating_sub(1)).step_by(2) {
                d.push((img.at(x + 1, y) - img.at(x, y)).abs());
            }
        }
        let noise = if d.is_empty() { 0.0 } else { 1.4826 * ip::median(&d) / std::f32::consts::SQRT_2 };
        Self { img, noise }
    }

    /// Over the rays on the image: the share with a step the same way, with an edge standing
    /// out from the slope of light either side, and with a step or an outline (a thin ring
    /// darker or brighter than both sides, all the way round the same); and the share of
    /// rays on the image.
    pub fn support(&self, c: &(f64, f64, f64)) -> Support {
        let (cx, cy, r) = *c;
        let (w, h) = (self.img.w as f64, self.img.h as f64);
        // Bands along each ray (px from the circle): a gap either side of the edge (room
        // for a blurred edge and a slightly-off fit), then two bands each side.
        let gap = (0.06 * r).max(4.0);
        let band = (0.08 * r).max(3.0);
        let n = (band.round() as usize).clamp(3, 40);
        let threshold = MIN_STEP.max(4.0 * self.noise / (n as f32).sqrt());
        // The outline's darkest or brightest point (3 px smoothed) stands out from noise too.
        let ring_threshold = MIN_STEP.max(3.0 * self.noise / 3f32.sqrt());
        // A straight slope of light changes edge / slope = (2 gap + band) / band times
        // as much across the edge as across one band; an edge must stand out well past that.
        let slope_ratio = SLOPE_MARGIN * ((2.0 * gap + band) / band) as f32;
        let mut edges = Vec::new();
        let mut on = 0;
        for k in 0..EDGE_RAYS {
            let a = k as f64 * 2.0 * PI / EDGE_RAYS as f64;
            let (dx, dy) = (a.cos(), a.sin());
            let at = |t: f64| (cx + t * dx, cy + t * dy);
            let (inner, outer) = (r - gap - 2.0 * band, r + gap + 2.0 * band);
            let (x0, y0) = at(inner.max(0.0));
            let (x1, y1) = at(outer);
            if [x0, x1].iter().any(|&x| x < 0.0 || x > w - 1.0) || [y0, y1].iter().any(|&y| y < 0.0 || y > h - 1.0) {
                continue;
            }
            on += 1;
            let value = |t: f64| {
                let (x, y) = at(t);
                ip::sample(&self.img, x as f32, y as f32)
            };
            let mean = |from: f64| (0..n).map(|i| value(from + band * (i as f64 + 0.5) / n as f64)).sum::<f32>() / n as f32;
            let (a_in, b_in) = (mean(inner), mean(r - gap - band));
            let (c_out, d_out) = (mean(r + gap), mean(r + gap + band));
            // Across the gap at 1 px steps, smoothed over 3: its darkest and brightest.
            let m = (2.0 * gap).round().clamp(3.0, 80.0) as usize;
            let line: Vec<f32> = (0..=m).map(|i| value(r - gap + 2.0 * gap * i as f64 / m as f64)).collect();
            let smooth: Vec<f32> = (1..line.len() - 1).map(|i| (line[i - 1] + line[i] + line[i + 1]) / 3.0).collect();
            let sides = (b_in + c_out) / 2.0;
            let dark = sides - smooth.iter().cloned().fold(f32::INFINITY, f32::min);
            let light = smooth.iter().cloned().fold(f32::NEG_INFINITY, f32::max) - sides;
            edges.push((b_in - c_out, (a_in - b_in).abs().max((c_out - d_out).abs()), dark, light));
        }
        if edges.is_empty() {
            return Support::default();
        }
        let sign = if ip::median(&edges.iter().map(|e| e.0).collect::<Vec<_>>()) >= 0.0 { 1.0 } else { -1.0 };
        let dark_ring = ip::median(&edges.iter().map(|e| e.2).collect::<Vec<_>>()) >= ip::median(&edges.iter().map(|e| e.3).collect::<Vec<_>>());
        let n = edges.len() as f64;
        let share = |f: &dyn Fn(&(f32, f32, f32, f32)) -> bool| edges.iter().filter(|e| f(e)).count() as f64 / n;
        let step = |e: &(f32, f32, f32, f32)| e.0 * sign >= threshold;
        let ring = |e: &(f32, f32, f32, f32)| (if dark_ring { e.2 } else { e.3 }) >= ring_threshold;
        Support {
            step: share(&step),
            sharp: share(&|e| step(e) && e.0 * sign >= slope_ratio * e.1),
            edge: share(&|e| step(e) || ring(e)),
            on: on as f64 / EDGE_RAYS as f64,
        }
    }

    fn on_image(&self, c: &(f64, f64, f64), s: &Support) -> bool {
        c.0 >= 0.0 && c.1 >= 0.0 && c.0 < self.img.w as f64 && c.1 < self.img.h as f64 && s.on >= ON_IMAGE
    }

    /// A clear cladding edge (for circles found by the extra searches).
    pub fn is_fiber(&self, c: &(f64, f64, f64)) -> bool {
        let s = self.support(c);
        self.on_image(c, &s) && s.step >= EDGE_SUPPORT && s.sharp >= SHARP_SUPPORT
    }

    /// Some edge, a step or an outline, round most of it (for the first search's circles).
    pub fn has_edge(&self, c: &(f64, f64, f64)) -> bool {
        let s = self.support(c);
        self.on_image(c, &s) && s.edge >= OUTLINE_SUPPORT
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Support {
    pub step: f64,
    pub sharp: f64,
    pub edge: f64,
    pub on: f64,
}

/// Fibers side by side are the same size: with one found, look for more circles
/// of that size (the best-edged one's) that the first search missed.
fn same_size_neighbours(gray: &image::GrayImage, check: &EdgeCheck, mut found: Vec<(f64, f64, f64)>, max_fibers: usize)
                        -> Vec<(f64, f64, f64)> {
    // The best-edged of the typical size (not an odd one out, e.g. a circle round a column of fibers).
    let mut radii: Vec<f64> = found.iter().map(|c| c.2).collect();
    radii.sort_by(|a, b| a.total_cmp(b));
    let typical = radii.get(radii.len() / 2).copied().unwrap_or(0.0);
    let Some(best) = found.iter().copied().filter(|c| c.2 < 1.25 * typical && c.2 > 0.8 * typical)
        .max_by(|a, b| check.support(a).sharp.partial_cmp(&check.support(b).sharp).unwrap()) else {
        return found;
    };
    if found.len() >= max_fibers {
        return found;
    }
    let r = best.2;
    let extra: Vec<(f64, f64)> = fixed_radius_centres(gray, r, max_fibers)
        .into_iter()
        // (a much smaller circle there may be a rod or core, which this fiber replaces below)
        .filter(|&(x, y)| found.iter().all(|f| f.2 < 0.8 * r || f.2 > 1.5 * r || (x - f.0).hypot(y - f.1) > 0.8 * (r + f.2)))
        .collect();
    let fits: Vec<Option<(f64, f64, f64)>> = std::thread::scope(|s| {
        let jobs: Vec<_> = extra.iter().map(|&(x, y)| s.spawn(move || fit_cladding_edge(gray, x, y, r, 360))).collect();
        jobs.into_iter().map(|j| j.join().unwrap()).collect()
    });
    for ((x, y), f) in extra.into_iter().zip(fits) {
        let c = match f {
            Some(f) if (f.2 - r).abs() < 0.1 * r && (f.0 - x).hypot(f.1 - y) < 0.2 * r => f,
            _ => continue,
        };
        if !check.is_fiber(&c) {
            continue;
        }
        // A much smaller circle inside it is one of its stress rods or its core, not a fiber.
        found.retain(|k| !(k.2 < 0.8 * c.2 && (c.0 - k.0).hypot(c.1 - k.1) < c.2));
        // (a much bigger circle round it isn't a fiber: prune_nested drops it afterwards)
        if found.len() < max_fibers && found.iter().all(|k| k.2 > 1.5 * c.2 || (c.0 - k.0).hypot(c.1 - k.1) > 0.8 * (c.2 + k.2)) {
            found.push(c);
        }
    }
    found
}

/// Below this share of the image's short side, a "fiber" may be just its bright core.
const SMALL_FIBER: f64 = 0.15;

/// With the cladding about as bright as the background, only a lit core stands out
/// and passes for a small fiber: if a faint circle (and no strong one) surrounds it,
/// that's the cladding. A strong circle could be the microscope's field stop instead.
fn faint_cladding_around(gray: &image::GrayImage, found: Vec<(f64, f64, f64)>) -> Vec<(f64, f64, f64)> {
    let small = SMALL_FIBER * gray.width().min(gray.height()) as f64;
    if found.is_empty() || found.iter().any(|c| c.2 >= small) || hough_circles(gray, 10, 800.0, None).iter().any(|h| h.2 >= small) {
        return found;
    }
    let inside = |c: &(f64, f64, f64), h: &(f64, f64, f64)| (c.0 - h.0).hypot(c.1 - h.1) + c.2 < h.2 && h.2 > 3.0 * c.2;
    let claddings: Vec<_> = refine_faint(gray, hough_circles(gray, 10, 400.0, Some(FAINT_EDGES))).into_iter()
        .filter(|h| found.iter().any(|c| inside(c, h)))
        .collect();
    if claddings.is_empty() {
        return found;
    }
    let mut kept: Vec<_> = found.into_iter().filter(|c| !claddings.iter().any(|h| inside(c, h))).collect();
    kept.extend(claddings);
    kept
}

/// Just the circle whose center is nearest the middle of a w x h image.
pub fn nearest_centre(circles: &[(f64, f64, f64)], w: u32, h: u32) -> Vec<(f64, f64, f64)> {
    let (mx, my) = (w as f64 / 2.0, h as f64 / 2.0);
    circles
        .iter()
        .min_by(|a, b| (a.0 - mx).hypot(a.1 - my).partial_cmp(&(b.0 - mx).hypot(b.1 - my)).unwrap())
        .map(|c| vec![*c])
        .unwrap_or_default()
}

fn otsu_mask(gray: &image::GrayImage) -> Mask {
    let blur = ip::gaussian_blur_u8(gray, (gray.width().min(gray.height()) as f64 / 300.0).max(1.0));
    let t = ip::otsu_u8(&blur);
    Mask { w: blur.width() as usize, h: blur.height() as usize, data: blur.as_raw().iter().map(|&v| v > t).collect() }
}

/// Fibers that touch each other or run off the image, the best seen first.
pub fn find_touching_fibers(gray: &image::GrayImage, max_fibers: usize) -> Vec<(f64, f64, f64)> {
    touching_fibers(gray, &otsu_mask(gray), max_fibers, FIRST_MIN_R)
}

/// `min_r`: the smallest fiber radius, share of the image's short side.
fn touching_fibers(gray: &image::GrayImage, bright: &Mask, max_fibers: usize, min_r: f64) -> Vec<(f64, f64, f64)> {
    let mask = ip::open(&ip::fill_holes(bright), &ip::ellipse_kernel(9));
    let depth = ip::distance_transform(&mask);
    let (x, y) = depth.argmax();
    let d = depth.at(x, y) as f64;
    if d < gray.width().min(gray.height()) as f64 * min_r {
        return Vec::new();
    }
    let Some(first) = fit_cladding_edge(gray, x as f64, y as f64, d, 360) else {
        return Vec::new();
    };
    let mut found = vec![first];
    for (cx, cy) in fixed_radius_centres(gray, first.2, max_fibers) {
        if found.iter().all(|f| (cx - f.0).hypot(cy - f.1) > 1.5 * first.2) {
            found.push((cx, cy, first.2));
        }
    }
    found.truncate(max_fibers);
    found
}

/// numpy.gradient along rows: central differences inside, one-sided at the ends.
fn gradient_row(v: &[f32]) -> Vec<f32> {
    let n = v.len();
    (0..n)
        .map(|i| {
            if n < 2 {
                0.0
            } else if i == 0 {
                v[1] - v[0]
            } else if i == n - 1 {
                v[n - 1] - v[n - 2]
            } else {
                (v[i + 1] - v[i - 1]) / 2.0
            }
        })
        .collect()
}

/// The cladding circle near (cx, cy, radius): per ray, the outermost fall from
/// above to below halfway between that ray's cladding level and the
/// background, moved to the steepest drop near it; a circle fitted to those.
/// The image blurred (8-bit rounding, as GaussianBlur on 8-bit images) only
/// where the fit will sample: a box round the circle, with a margin of the
/// kernel's reach, so the values are those of blurring the whole image.
struct Smoothed {
    x0: usize,
    y0: usize,
    img: ImgF,
}

fn smooth_region(gray: &image::GrayImage, kernel: &[f32], cx: f64, cy: f64, reach: f64) -> Smoothed {
    let (w, h) = (gray.width() as i64, gray.height() as i64);
    let kr = (kernel.len() / 2) as i64;
    let x0 = ((cx - reach) as i64 - kr - 2).clamp(0, w - 1);
    let y0 = ((cy - reach) as i64 - kr - 2).clamp(0, h - 1);
    let x1 = ((cx + reach) as i64 + kr + 3).clamp(x0 + 1, w);
    let y1 = ((cy + reach) as i64 + kr + 3).clamp(y0 + 1, h);
    // Blur the box plus the kernel's reach (from the image where it exists, mirrored at its edges),
    // then keep the box.
    let (bx0, by0) = ((x0 - kr).max(0), (y0 - kr).max(0));
    let (bx1, by1) = ((x1 + kr).min(w), (y1 + kr).min(h));
    let sub = image::imageops::crop_imm(gray, bx0 as u32, by0 as u32, (bx1 - bx0) as u32, (by1 - by0) as u32).to_image();
    let blurred = ip::sep_filter(&ImgF::from_gray(&sub), kernel, kernel);
    let img = ImgF::from_fn((x1 - x0) as usize, (y1 - y0) as usize, |x, y| {
        ip::sat_u8(blurred.at(x + (x0 - bx0) as usize, y + (y0 - by0) as usize)) as f32
    });
    Smoothed { x0: x0 as usize, y0: y0 as usize, img }
}

/// The 5th percentile of the blurred image's every 4th pixel (both ways): the background level.
fn background_level(gray: &image::GrayImage, kernel: &[f32]) -> f32 {
    let (w, h) = (gray.width() as usize, gray.height() as usize);
    let raw = gray.as_raw();
    let kr = kernel.len() / 2;
    let rows: Vec<usize> = (0..h).step_by(4).collect();
    let cols: Vec<usize> = (0..w).step_by(4).collect();
    // Down the columns first (only the rows needed), then along those rows (only the columns needed).
    let mut down = vec![0f32; rows.len() * w];
    ip::par_rows(&mut down, w, |i, row| {
        let y = rows[i];
        for (k, &c) in kernel.iter().enumerate() {
            let yy = ip::reflect101(y as isize + k as isize - kr as isize, h);
            for (o, &v) in row.iter_mut().zip(&raw[yy * w..(yy + 1) * w]) {
                *o += c * v as f32;
            }
        }
    });
    let mut values = Vec::with_capacity(rows.len() * cols.len());
    for i in 0..rows.len() {
        let row = &down[i * w..(i + 1) * w];
        for &x in &cols {
            let mut acc = 0f32;
            for (k, &c) in kernel.iter().enumerate() {
                acc += c * row[ip::reflect101(x as isize + k as isize - kr as isize, w)];
            }
            values.push(ip::sat_u8(acc) as f32);
        }
    }
    ip::percentile(&values, 5.0)
}

pub fn fit_cladding_edge(gray: &image::GrayImage, cx: f64, cy: f64, radius: f64, rays: usize) -> Option<(f64, f64, f64)> {
    let kernel = ip::gaussian_kernel((radius / 150.0).max(1.5), true);
    let (w, h) = (gray.width() as f32, gray.height() as f32);
    let background = background_level(gray, &kernel);
    let angles = linspace(0.0, 2.0 * PI, rays, false);
    let (mut cx, mut cy, mut radius) = (cx, cy, radius);
    let mut smooth: Option<(Smoothed, f64, f64, f64)> = None; // and the circle it covers to 1.6 r
    for _ in 0..4 {
        // Blurred round the circle (again only if it has moved out of what's blurred).
        let covered = smooth.as_ref().is_some_and(|(_, sx, sy, sr)| (cx - sx).hypot(cy - sy) + 1.35 * radius + 2.0 <= 1.6 * sr);
        if !covered {
            smooth = Some((smooth_region(gray, &kernel, cx, cy, 1.6 * radius), cx, cy, radius));
        }
        let region = &smooth.as_ref().unwrap().0;
        let (ox, oy) = (region.x0 as f32, region.y0 as f32);
        let t = arange(0.5 * radius, 1.35 * radius, 1.0);
        let nt = t.len();
        if nt < 3 {
            return None;
        }
        let mid: Vec<usize> = (0..nt).filter(|&j| t[j] >= 0.6 * radius && t[j] <= 0.85 * radius).collect();
        let mut levels = Vec::with_capacity(rays);
        let mut rows = Vec::with_capacity(rays);
        for &a in &angles {
            let (ca, sa) = (a.cos(), a.sin());
            let xs: Vec<f32> = t.iter().map(|&tt| (cx + ca * tt) as f32).collect();
            let ys: Vec<f32> = t.iter().map(|&tt| (cy + sa * tt) as f32).collect();
            let inside: Vec<bool> = xs.iter().zip(&ys).map(|(&x, &y)| x >= 0.0 && x < w - 1.0 && y >= 0.0 && y < h - 1.0).collect();
            let profile: Vec<f32> = xs
                .iter()
                .zip(&ys)
                .map(|(&x, &y)| if x >= 0.0 && y >= 0.0 && x < w && y < h { ip::sample(&region.img, x - ox, y - oy) } else { 0.0 })
                .collect();
            let level = ip::median(&mid.iter().map(|&j| profile[j]).collect::<Vec<_>>());
            levels.push(level);
            rows.push((xs, ys, inside, profile));
        }
        let median_level = ip::median(&levels);
        let window = (0.06 * radius).max(3.0) as isize;
        let (mut px, mut py) = (Vec::new(), Vec::new());
        for (i, (xs, ys, inside, profile)) in rows.iter().enumerate() {
            let level = levels[i];
            let half = (level + background) / 2.0;
            let above: Vec<bool> = profile.iter().zip(inside).map(|(&p, &ins)| p > half && ins).collect();
            let slope = gradient_row(profile);
            // erode with a 1x5 kernel: the smallest slope within 2 samples
            let eroded: Vec<f32> = (0..nt).map(|j| {
                let lo = j.saturating_sub(2);
                let hi = (j + 2).min(nt - 1);
                slope[lo..=hi].iter().cloned().fold(f32::INFINITY, f32::min)
            }).collect();
            let mut falls: Vec<bool> = (0..nt - 1).map(|j| above[j] && !above[j + 1] && inside[j + 1]).collect();
            let drop: Vec<f32> = (0..nt - 1).map(|j| if falls[j] { -eroded[j] } else { 0.0 }).collect();
            let top = drop.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            for j in 0..nt - 1 {
                falls[j] = falls[j] && drop[j] >= FALL_SHARE * top;
            }
            let usable = falls.iter().any(|&f| f) && level - background > 0.25 * (median_level - background);
            if !usable {
                continue;
            }
            let last = (0..nt - 1).rev().find(|&j| falls[j]).unwrap() as isize;
            let mut best = (f32::INFINITY, last);
            for d in -window..=window {
                let k = last + d;
                let s = if k >= 0 && (k as usize) < nt { slope[k as usize] } else { f32::INFINITY };
                if s < best.0 {
                    best = (s, k);
                }
            }
            let k = if best.0.is_finite() { best.1 as usize } else { (last - window).clamp(0, nt as isize - 1) as usize };
            px.push(xs[k] as f64);
            py.push(ys[k] as f64);
        }
        if px.len() < 20 {
            return None;
        }
        let fit = fit_circle(&px, &py, 20)?;
        (cx, cy, radius) = fit;
    }
    Some((cx, cy, radius))
}

/// Least-squares circle through the points, leaving out stray ones.
pub fn fit_circle(px: &[f64], py: &[f64], min_points: usize) -> Option<(f64, f64, f64)> {
    let n = px.len();
    let mut keep = vec![true; n];
    let (mut a, mut b, mut r) = (0.0, 0.0, 0.0);
    for _ in 0..5 {
        let idx: Vec<usize> = (0..n).filter(|&i| keep[i]).collect();
        if idx.len() < min_points {
            return None;
        }
        let rows: Vec<Vec<f64>> = idx.iter().map(|&i| vec![2.0 * px[i], 2.0 * py[i], 1.0]).collect();
        let rhs: Vec<f64> = idx.iter().map(|&i| px[i] * px[i] + py[i] * py[i]).collect();
        let s = ip::lstsq(&rows, &rhs)?;
        (a, b) = (s[0], s[1]);
        r = (s[2] + a * a + b * b).max(1.0).sqrt();
        let off: Vec<f64> = (0..n).map(|i| (px[i] - a).hypot(py[i] - b) - r).collect();
        let spread = 1.4826 * ip::median_f64(&idx.iter().map(|&i| off[i].abs()).collect::<Vec<_>>()) + 0.5;
        for i in 0..n {
            keep[i] = off[i].abs() < 2.5 * spread;
        }
    }
    Some((a, b, r))
}

/// Centers of circles of this radius (also off the image), strongest first.
fn fixed_radius_centres(gray: &image::GrayImage, radius: f64, max_fibers: usize) -> Vec<(f64, f64)> {
    let scale = (60.0 / radius).min(1.0);
    let (w, h) = (gray.width() as f64, gray.height() as f64);
    let (sw, sh) = (rne(w * scale) as usize, rne(h * scale) as usize);
    let mut small = ImgF::from_gray(gray);
    if (sw, sh) != (small.w, small.h) {
        small = ImgF::from_gray(&ip::resize_area(&small, sw, sh).to_gray());
    }
    let small = ip::gaussian_blur(&small, 1.5);
    let gx = ip::sobel(&small, true);
    let gy = ip::sobel(&small, false);
    let strength: Vec<f32> = gx.data.iter().zip(&gy.data).map(|(a, b)| a.hypot(*b)).collect();
    let thresh = ip::percentile(&strength, 90.0);
    let r = radius * scale;
    let pad = (r * 1.1) as usize;
    let (vw, vh) = (small.w + 2 * pad, small.h + 2 * pad);
    let mut votes = ImgF::new(vw, vh);
    let rrs = linspace(0.97 * r, 1.03 * r, 5, true);
    for i in 0..strength.len() {
        let s = strength[i];
        if s <= thresh {
            continue;
        }
        let (x, y) = ((i % small.w) as f64, (i / small.w) as f64);
        let (ux, uy) = (gx.data[i] as f64 / s as f64, gy.data[i] as f64 / s as f64);
        for &rr in &rrs {
            let vx = rne(x + ux * rr) as isize + pad as isize;
            let vy = rne(y + uy * rr) as isize + pad as isize;
            if vx >= 0 && vy >= 0 && (vx as usize) < vw && (vy as usize) < vh {
                votes.data[vy as usize * vw + vx as usize] += s;
            }
        }
    }
    let mut votes = ip::gaussian_blur(&votes, (r * 0.03).max(1.0));
    let top = votes.max_value();
    let mut found = Vec::new();
    while found.len() < max_fibers && top > 0.0 {
        let (x, y) = votes.argmax();
        if votes.at(x, y) < 0.12 * top {
            break;
        }
        found.push(((x as f64 - pad as f64) / scale, (y as f64 - pad as f64) / scale));
        let rad = (r * 1.6) as isize;
        for dy in -rad..=rad {
            for dx in -rad..=rad {
                let (xx, yy) = (x as isize + dx, y as isize + dy);
                if dx * dx + dy * dy <= rad * rad && xx >= 0 && yy >= 0 && (xx as usize) < vw && (yy as usize) < vh {
                    votes.data[yy as usize * vw + xx as usize] = 0.0;
                }
            }
        }
    }
    found
}

fn round_regions(gray: &image::GrayImage, bright: &Mask, min_share: f64) -> Vec<(f64, f64, f64)> {
    let (w, h) = (gray.width() as f64, gray.height() as f64);
    let min_r = w.min(h) * min_share;
    let mut found = Vec::new();
    for mask in [bright.clone(), bright.not()] {
        for c in ip::outer_contours(&mask) {
            let area = ip::contour_area(&c);
            let perimeter = ip::arc_length(&c);
            if area < PI * min_r * min_r || perimeter == 0.0 {
                continue;
            }
            let (x, y, r) = ip::min_enclosing_circle(&c);
            let roundness = 4.0 * PI * area / (perimeter * perimeter);
            let fill = area / (PI * r * r);
            let touches_edge = x - r < 1.0 || y - r < 1.0 || x + r > w - 1.0 || y + r > h - 1.0;
            if roundness > 0.75 && fill > 0.8 && !touches_edge {
                found.push((x, y, (area / PI).sqrt()));
            }
        }
        if !found.is_empty() {
            break;
        }
    }
    found.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    found
}

/// Circles by edges voting along their gradient for centers (as
/// cv2.HoughCircles HOUGH_GRADIENT does), on a smaller copy. The last resort.
/// A fiber on a bright background (lit from the front, or grey) has only a faint
/// edge, below the usual edge strength: then look again on a smaller, smoother
/// copy, taking this image's strongest edges, whatever their strength.
fn find_hough_circles(gray: &image::GrayImage, max_fibers: usize) -> Vec<(f64, f64, f64)> {
    let found = hough_circles(gray, max_fibers, 800.0, None);
    if !found.is_empty() {
        return found;
    }
    refine_faint(gray, hough_circles(gray, max_fibers, 400.0, Some(FAINT_EDGES)))
}

/// The faint pass's circles are rough (found on a small copy): fit each to its cladding edge.
fn refine_faint(gray: &image::GrayImage, circles: Vec<(f64, f64, f64)>) -> Vec<(f64, f64, f64)> {
    circles.into_iter().map(|c| match fit_cladding_edge(gray, c.0, c.1, c.2, 360) {
        Some(f) if (f.2 - c.2).abs() < 0.2 * c.2 && (f.0 - c.0).hypot(f.1 - c.1) < 0.2 * c.2 => f,
        _ => c,
    }).collect()
}

/// Faint-edge pass: the strongest this share of the image counts as edges (if stronger than noise).
const FAINT_EDGES: f64 = 0.04;
const FAINT_EDGE_MIN: f32 = 6.0;

/// `faint`: None = edges of fixed strength; Some(share) = this image's strongest `share` of gradients.
fn hough_circles(gray: &image::GrayImage, max_fibers: usize, size: f64, faint: Option<f64>) -> Vec<(f64, f64, f64)> {
    hough_circles_sized(gray, max_fibers, size, faint, 0.06, 0.15)
}

/// `min_r`, `min_dist`: the smallest radius and the closest centers, shares of the short side.
fn hough_circles_sized(gray: &image::GrayImage, max_fibers: usize, size: f64, faint: Option<f64>, min_share: f64, dist_share: f64)
                       -> Vec<(f64, f64, f64)> {
    let (w, h) = (gray.width() as f64, gray.height() as f64);
    let scale = (size / w.max(h)).min(1.0);
    let small = ip::resize_gray(gray, rne(w * scale) as u32, rne(h * scale) as u32, ip::Interp::Area);
    let small = ip::median_blur(&small, 5);
    let f = ImgF::from_gray(&small);
    let (gx, gy) = (ip::sobel(&f, true), ip::sobel(&f, false));
    let threshold = match faint {
        None => 120.0, // Canny's high threshold, roughly
        Some(share) => {
            let mags: Vec<f32> = gx.data.iter().zip(&gy.data).map(|(a, b)| a.hypot(*b)).collect();
            ip::percentile(&mags, 100.0 * (1.0 - share)).max(FAINT_EDGE_MIN) as f64
        }
    };
    let short = small.width().min(small.height()) as f64;
    let (min_r, max_r) = (rne(short * min_share).max(4.0), rne(short * 0.5));
    let dp = 1.5;
    let (aw, ah) = ((f.w as f64 / dp).ceil() as usize + 1, (f.h as f64 / dp).ceil() as usize + 1);
    let mut acc = ImgF::new(aw, ah);
    let mut edges = Vec::new();
    for y in 1..f.h - 1 {
        for x in 1..f.w - 1 {
            let (dx, dy) = (gx.at(x, y) as f64, gy.at(x, y) as f64);
            let mag = dx.hypot(dy);
            if mag < threshold {
                continue;
            }
            edges.push((x as f64, y as f64));
            let (ux, uy) = (dx / mag, dy / mag);
            for sign in [-1.0, 1.0] {
                let mut r = min_r;
                while r <= max_r {
                    let (cx, cy) = ((x as f64 + sign * ux * r) / dp, (y as f64 + sign * uy * r) / dp);
                    if cx >= 0.0 && cy >= 0.0 && (cx as usize) < aw && (cy as usize) < ah {
                        acc.data[cy as usize * aw + cx as usize] += 1.0;
                    }
                    r += dp;
                }
            }
        }
    }
    // Centers: local maxima above the accumulator threshold, at least minDist apart.
    let min_dist = short * dist_share;
    let mut peaks: Vec<(f32, usize, usize)> = Vec::new();
    for y in 1..ah - 1 {
        for x in 1..aw - 1 {
            let v = acc.at(x, y);
            if v > 40.0 && v > acc.at(x - 1, y) && v >= acc.at(x + 1, y) && v > acc.at(x, y - 1) && v >= acc.at(x, y + 1) {
                peaks.push((v, x, y));
            }
        }
    }
    peaks.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let mut circles: Vec<(f64, f64, f64)> = Vec::new();
    for (_, x, y) in peaks {
        let (cx, cy) = ((x as f64 + 0.5) * dp, (y as f64 + 0.5) * dp);
        if circles.iter().any(|c| (c.0 - cx).hypot(c.1 - cy) < min_dist) {
            continue;
        }
        // The radius most edge points agree on.
        let mut hist = vec![0u32; max_r as usize + 2];
        for &(ex, ey) in &edges {
            let d = (ex - cx).hypot(ey - cy);
            if d >= min_r && d <= max_r {
                hist[d as usize] += 1;
            }
        }
        let (best_r, n) = hist.iter().enumerate().max_by_key(|e| e.1).map(|(r, &n)| (r, n)).unwrap();
        if n as f64 > 0.3 * best_r as f64 {
            circles.push((cx, cy, best_r as f64 + 0.5));
        }
    }
    circles.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    let mut found: Vec<(f64, f64, f64)> = Vec::new();
    for (x, y, r) in circles {
        let (x, y, r) = (x / scale, y / scale, r / scale);
        if found.iter().any(|f| (x - f.0).hypot(y - f.1) < f.2) {
            continue;
        }
        found.push((x, y, r));
        if found.len() == max_fibers {
            break;
        }
    }
    found
}

// --- stress parts by contrast -----------------------------------------------------------

/// How different each pixel is from the cladding, in noise units, and the ring it's measured in.
pub fn contrast_map(crop: &RgbImage, cx: f64, cy: f64, radius: f64, detection: &Detection) -> (ImgF, Mask) {
    let (w, h) = (crop.width() as usize, crop.height() as usize);
    let uv = |x: usize, y: usize| ((x as f64 - cx) / radius, (y as f64 - cy) / radius);
    let ring = Mask::from_fn(w, h, |x, y| {
        let (u, v) = uv(x, y);
        let d = u.hypot(v);
        d > CORE_FRACTION && d < EDGE_FRACTION
    });
    let n_ring = ring.count();
    if n_ring < 50 {
        return (ImgF::new(w, h), ring);
    }
    let lab = ip::to_lab(crop);
    let blur = radius * detection.smoothing / 100.0;
    let surface = |x: usize, y: usize| {
        let (u, v) = uv(x, y);
        [1.0, u, v, u * u + v * v]
    };
    let step = ((n_ring as f64 / FIT_SAMPLES).sqrt() as usize).max(1);
    let sx: Vec<usize> = (0..w).step_by(step).collect();
    let sy: Vec<usize> = (0..h).step_by(step).collect();
    let channels = LOOK_AT.iter().find(|l| l.0 == detection.look_at).unwrap_or(&LOOK_AT[0]).2;
    let mut score = ImgF::new(w, h);
    for &c in channels {
        let channel = if blur >= 0.3 { ip::gaussian_blur(&lab[c], blur) } else { lab[c].clone() };
        // The sparse grid: values, ring membership, surface terms.
        let mut values = Vec::new();
        let mut cells = Vec::new(); // (value, in ring, surface)
        for &y in &sy {
            for &x in &sx {
                let v = channel.at(x, y) as f64;
                let r = ring.at(x, y);
                if r {
                    values.push(v);
                }
                cells.push((v, r, surface(x, y)));
            }
        }
        // The commonest value in the ring (a histogram with 1-wide bins, smoothed by 3).
        let (lo, hi) = values.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| (a.min(v), b.max(v)));
        let bins = ((hi - lo) as usize).max(1);
        let (elo, ehi) = (lo, hi + 1.0);
        let mut hist = vec![0f64; bins];
        for &v in &values {
            let k = (((v - elo) / (ehi - elo)) * bins as f64) as usize;
            hist[k.min(bins - 1)] += 1.0;
        }
        let smoothed: Vec<f64> = (0..bins)
            .map(|i| hist[i] + if i > 0 { hist[i - 1] } else { 0.0 } + if i + 1 < bins { hist[i + 1] } else { 0.0 })
            .collect();
        let peak = (0..bins).fold(0, |b, i| if smoothed[i] > smoothed[b] { i } else { b });
        let common = elo + (ehi - elo) * peak as f64 / bins as f64;
        // Noise from neighbouring sample differences along rows.
        let gw = sx.len();
        let mut diffs = Vec::new();
        for (row, _) in sy.iter().enumerate() {
            for col in 1..gw {
                let (a, b) = (&cells[row * gw + col - 1], &cells[row * gw + col]);
                if b.1 {
                    diffs.push((b.0 - a.0).abs());
                }
            }
        }
        let step_noise = 1.4826 * ip::median_f64(&diffs) / 2f64.sqrt();
        let mut keep: Vec<bool> = cells.iter().map(|c| c.1 && (c.0 - common).abs() < (3.0 * step_noise).max(4.0 * NOISE_FLOOR)).collect();
        let mut coef = [0.0; 4];
        let mut noise = NOISE_FLOOR;
        for _ in 0..5 {
            let idx: Vec<usize> = (0..cells.len()).filter(|&i| keep[i]).collect();
            if idx.len() < 4 {
                break;
            }
            let rows: Vec<Vec<f64>> = idx.iter().map(|&i| cells[i].2.to_vec()).collect();
            let rhs: Vec<f64> = idx.iter().map(|&i| cells[i].0).collect();
            if let Some(s) = ip::lstsq(&rows, &rhs) {
                coef = [s[0], s[1], s[2], s[3]];
            }
            let off: Vec<f64> = cells.iter().map(|c| c.0 - (0..4).map(|k| c.2[k] * coef[k]).sum::<f64>()).collect();
            noise = 1.4826 * ip::median_f64(&idx.iter().map(|&i| off[i].abs()).collect::<Vec<_>>()) + NOISE_FLOOR;
            for i in 0..cells.len() {
                keep[i] = cells[i].1 && off[i].abs() < 2.0 * noise;
            }
        }
        ip::par_rows(&mut score.data, w, |y, row| {
            for (x, v) in row.iter_mut().enumerate() {
                let s = surface(x, y);
                let fit: f64 = (0..4).map(|k| s[k] * coef[k]).sum();
                let d = (channel.at(x, y) as f64 - fit) / noise;
                *v += (d * d) as f32;
            }
        });
    }
    (score.map(f32::sqrt), ring)
}

// --- the rod search (Panda) -------------------------------------------------------------

/// The fiber scaled to a radius of SYMMETRY_SIZE px, as blurred gradients with the
/// cladding edge and anything off the image zeroed.
pub struct Grads {
    gx: ImgF,
    gy: ImgF,
    half: usize,
    scale: f64,
    typical: f32,
    dirt: Mask,
    /// A clearly visible core spot (search scale), if there is one.
    core: Option<(f64, f64)>,
    /// 1 where the search map shows the image, 0 off it (fibers running off the frame).
    valid: ImgF,
}

fn rod_gradients(image: &RgbImage, cx: f64, cy: f64, radius: f64) -> Grads {
    let gray = ImgF::from_gray(&ip::to_gray(image));
    let scale = SYMMETRY_SIZE / radius;
    let half = (SYMMETRY_SIZE * 1.05) as usize;
    let to_crop: ip::Affine = [[scale, 0.0, half as f64 - cx * scale], [0.0, scale, half as f64 - cy * scale]];
    let size = 2 * half + 1;
    let crop = ip::warp_affine(&gray, &to_crop, size, size, false, false);
    let edge = Mask::from_fn(size, size, |x, y| {
        (x as f64 - half as f64).hypot(y as f64 - half as f64) > ROD_SEARCH_EDGE * SYMMETRY_SIZE
    });
    let dirt = dirt_mask(&crop, &edge.not());
    let core = detect_core(&crop, half);
    let crop = ip::gaussian_blur(&crop, 1.5);
    let gx = ip::sobel(&crop, true).map(|v| v / 8.0);
    let gy = ip::sobel(&crop, false).map(|v| v / 8.0);
    // Parts of the fiber off the image (nearest-pixel map of the image area, eroded 7x7).
    let inv = ip::invert_affine(&to_crop);
    let in_image = Mask::from_fn(size, size, |x, y| {
        let sx = inv[0][0] * x as f64 + inv[0][1] * y as f64 + inv[0][2];
        let sy = inv[1][0] * x as f64 + inv[1][1] * y as f64 + inv[1][2];
        let (ix, iy) = (sx.round(), sy.round());
        ix >= 0.0 && iy >= 0.0 && ix < gray.w as f64 && iy < gray.h as f64
    });
    let in_image = ip::erode(&in_image, &ip::rect_kernel(7, 7));
    let off = edge.or(&in_image.not());
    let mags: Vec<f32> = (0..gx.data.len()).filter(|&i| !off.data[i]).map(|i| gx.data[i].hypot(gy.data[i])).collect();
    let typical = ip::median(&mags).max(GRADIENT_FLOOR);
    let (mut gx, mut gy) = (gx, gy);
    for i in 0..off.data.len() {
        if off.data[i] {
            gx.data[i] = 0.0;
            gy.data[i] = 0.0;
        }
    }
    let valid = ImgF { w: size, h: size, data: in_image.data.iter().map(|&v| if v { 1.0 } else { 0.0 }).collect() };
    Grads { gx, gy, half, scale, typical, dirt, core, valid }
}

/// The fiber core, if it shows as a small spot (brighter or darker) near the
/// middle: a compact peak of a difference of Gaussians, standing out clearly
/// from the cladding's texture. Rod edges and shading aren't compact, so they
/// don't count. (crop: the fiber at the search scale, center at (half, half).)
fn detect_core(crop: &ImgF, half: usize) -> Option<(f64, f64)> {
    let d = ip::gaussian_blur(crop, 3.0).zip(&ip::gaussian_blur(crop, 10.0), |a, b| a - b);
    let (h, size) = (half as f64, SYMMETRY_SIZE);
    let dist = |x: usize, y: usize| (x as f64 - h).hypot(y as f64 - h);
    let reach = (CORE_SEARCH * size) as usize + 1;
    let mut best = (0f32, half, half);
    for y in half - reach..=half + reach {
        for x in half - reach..=half + reach {
            if dist(x, y) <= CORE_SEARCH * size && d.at(x, y).abs() > best.0 {
                best = (d.at(x, y).abs(), x, y);
            }
        }
    }
    let (peak, px, py) = best;
    // Noise: the robust spread of the difference over the inner fiber.
    let inner: Vec<f32> = (0..d.data.len())
        .filter(|&i| dist(i % d.w, i / d.w) < 0.6 * size)
        .map(|i| d.data[i])
        .collect();
    let med = ip::median(&inner);
    let noise = (1.4826 * ip::median(&inner.iter().map(|v| (v - med).abs()).collect::<Vec<_>>())).max(0.5);
    if peak < CORE_SIGNIFICANCE * noise {
        return None;
    }
    // Compact: strong right round the peak, weak in a ring further out.
    let sign = d.at(px, py).signum();
    let (mut inside, mut ni, mut ring, mut nr) = (0f32, 0, 0f32, 0);
    for y in py.saturating_sub(20)..(py + 21).min(d.h) {
        for x in px.saturating_sub(20)..(px + 21).min(d.w) {
            let r = (x as f64 - px as f64).hypot(y as f64 - py as f64);
            if r <= 5.0 {
                inside += sign * d.at(x, y);
                ni += 1;
            } else if (12.0..=20.0).contains(&r) {
                ring += sign * d.at(x, y);
                nr += 1;
            }
        }
    }
    let (inside, ring) = (inside / ni.max(1) as f32, ring / nr.max(1) as f32);
    if inside < 3.0 * ring.max(0.0) || inside < 0.5 * peak {
        return None;
    }
    // Its center: the weighted middle of the strong part.
    let (mut sx, mut sy, mut sw) = (0f64, 0f64, 0f64);
    for y in py.saturating_sub(8)..(py + 9).min(d.h) {
        for x in px.saturating_sub(8)..(px + 9).min(d.w) {
            let v = (sign * d.at(x, y)) as f64 - 0.5 * peak as f64;
            if v > 0.0 {
                sx += v * x as f64;
                sy += v * y as f64;
                sw += v;
            }
        }
    }
    (sw > 0.0).then(|| (sx / sw, sy / sw))
}

/// Dust, specks and streaks: pixels much brighter or darker than the median round them, widened.
fn dirt_mask(crop: &ImgF, inside: &Mask) -> Mask {
    let as_u8 = image::GrayImage::from_raw(crop.w as u32, crop.h as u32,
        crop.data.iter().map(|&v| v.clamp(0.0, 255.0) as u8).collect()).unwrap(); // astype(uint8): truncates
    let smooth = ImgF::from_gray(&ip::median_blur(&as_u8, DIRT_WINDOW));
    let off = crop.zip(&smooth, |a, b| (a - b).abs());
    let inside_off: Vec<f32> = (0..off.data.len()).filter(|&i| inside.data[i]).map(|i| off.data[i]).collect();
    let noise = (1.4826 * ip::median(&inside_off)).max(2.0);
    let dirt = Mask { w: crop.w, h: crop.h, data: (0..off.data.len()).map(|i| off.data[i] > DIRT_CONTRAST * noise && inside.data[i]).collect() };
    ip::dilate(&dirt, &ip::ellipse_kernel(7))
}

/// Mean round the circle of the capped gradient across it.
/// Mean round the circle of the capped gradient across it, over the part of
/// the circle on the image (at least MIN_SHOWN of it counts, so a circle
/// mostly off the image can't score high on a scrap of edge).
fn ring_score(gx: &ImgF, gy: &ImgF, valid: &ImgF, cap: f32, qx: f64, qy: f64, rho: f64, trig: &[(f64, f64)], exact: bool) -> f32 {
    let (mut total, mut shown) = (0f32, 0f32);
    for &(ca, sa) in trig {
        let (x, y) = (qx + rho * ca, qy + rho * sa);
        let (sx, sy, v) = if exact {
            let (fx, fy) = (x as f32, y as f32);
            (ip::sample(gx, fx, fy), ip::sample(gy, fx, fy), ip::sample(valid, fx, fy))
        } else {
            let xi = (rne(x) as isize).clamp(0, gx.w as isize - 1) as usize;
            let yi = (rne(y) as isize).clamp(0, gx.h as isize - 1) as usize;
            (gx.at(xi, yi), gy.at(xi, yi), valid.at(xi, yi))
        };
        total += (sx * ca as f32 + sy * sa as f32).abs().min(cap);
        shown += v;
    }
    total / shown.max(MIN_SHOWN * trig.len() as f32)
}

fn trig_table(samples: usize) -> Vec<(f64, f64)> {
    linspace(0.0, 2.0 * PI, samples, false).into_iter().map(|a| (a.cos(), a.sin())).collect()
}

fn trig(samples: usize) -> &'static [(f64, f64)] {
    static T128: OnceLock<Vec<(f64, f64)>> = OnceLock::new();
    static T256: OnceLock<Vec<(f64, f64)>> = OnceLock::new();
    match samples {
        128 => T128.get_or_init(|| trig_table(128)),
        256 => T256.get_or_init(|| trig_table(256)),
        _ => panic!("no table for {samples}"),
    }
}

/// A rod pair: midpoint x, y (search scale), angle (radians), spacing, rod radius.
type Pair = [f64; 5];

fn pair_score(g: (&ImgF, &ImgF), valid: &ImgF, cap: f32, p: &Pair, samples: usize, exact: bool, add: bool) -> f32 {
    let (dx, dy) = (p[3] * p[2].cos(), -p[3] * p[2].sin()); // screen y points down
    let t = trig(samples);
    let a = ring_score(g.0, g.1, valid, cap, p[0] + dx, p[1] + dy, p[4], t, exact);
    let b = ring_score(g.0, g.1, valid, cap, p[0] - dx, p[1] - dy, p[4], t, exact);
    if add { a + b } else { a.min(b) }
}

struct PairGrid {
    angles: Vec<f64>,
    spacings: Vec<f64>,
    radii: Vec<usize>,
    sizes: Vec<f64>,
    ox: Vec<f64>,
    oy: Vec<f64>,
    inside: Vec<bool>, // [pair][offset]
    first: Vec<usize>,
    second: Vec<usize>,
    box_: (usize, usize, usize, usize),
}

fn pair_grid(half: usize) -> &'static PairGrid {
    static GRID: OnceLock<(usize, PairGrid)> = OnceLock::new();
    let (h, g) = GRID.get_or_init(|| (half, make_pair_grid(half)));
    assert_eq!(*h, half);
    g
}

fn make_pair_grid(half: usize) -> PairGrid {
    let size = SYMMETRY_SIZE;
    let sizes: Vec<f64> = arange(ROD_SIZES.0, ROD_SIZES.1, 0.015).into_iter().map(|v| v * size).collect();
    let angle_list: Vec<f64> = arange(0.0, 180.0, 2.0).into_iter().map(f64::to_radians).collect();
    let spacing_list: Vec<f64> = arange(ROD_SPACINGS.0, ROD_SPACINGS.1, 0.02).into_iter().map(|v| v * size).collect();
    let (mut angles, mut spacings, mut radii) = (Vec::new(), Vec::new(), Vec::new());
    for &a in &angle_list {
        for &s in &spacing_list {
            for r in 0..sizes.len() {
                if s - sizes[r] > 0.05 * size {
                    angles.push(a);
                    spacings.push(s);
                    radii.push(r);
                }
            }
        }
    }
    let offsets: Vec<f64> = arange(-0.08, 0.081, 0.02);
    let (mut ox, mut oy) = (Vec::new(), Vec::new());
    for &y in &offsets {
        for &x in &offsets {
            ox.push(x * size);
            oy.push(y * size);
        }
    }
    let n_off = ox.len();
    let reach = (ROD_MAX_REACH * size - sizes[0]).ceil() as usize + 1;
    let (lo, hi) = (half - reach, half + reach + 1);
    let w = hi - lo;
    let index = |r: usize, x: f64, y: f64| {
        let xi = (rne(x) as isize - lo as isize).clamp(0, w as isize - 1) as usize;
        let yi = (rne(y) as isize - lo as isize).clamp(0, w as isize - 1) as usize;
        (r * w + yi) * w + xi
    };
    let n = angles.len();
    let (mut inside, mut first, mut second) = (vec![false; n * n_off], vec![0; n * n_off], vec![0; n * n_off]);
    for i in 0..n {
        let (dx, dy) = (spacings[i] * angles[i].cos(), -spacings[i] * angles[i].sin());
        for o in 0..n_off {
            let k = i * n_off + o;
            inside[k] = spacings[i] + sizes[radii[i]] + ox[o].hypot(oy[o]) < ROD_MAX_REACH * size;
            let (mx, my) = (half as f64 + ox[o], half as f64 + oy[o]);
            first[k] = index(radii[i], mx + dx, my + dy);
            second[k] = index(radii[i], mx - dx, my - dy);
        }
    }
    PairGrid { angles, spacings, radii, sizes, ox, oy, inside, first, second, box_: (lo, lo, hi, hi) }
}

/// ring_score of circles of each radius centered on every pixel of the box (nearest-pixel samples).
fn ring_maps(gx: &ImgF, gy: &ImgF, valid: &ImgF, cap: f32, radii: &[f64], samples: usize, b: (usize, usize, usize, usize)) -> Vec<f32> {
    let (x0, y0, x1, y1) = b;
    let (bw, bh) = (x1 - x0, y1 - y0);
    let pad = radii.iter().cloned().fold(0.0, f64::max).ceil() as usize + 1;
    let (pw, ph) = (gx.w + 2 * pad, gx.h + 2 * pad);
    let mut maps = vec![0f32; radii.len() * bw * bh];
    let partial = valid.data.iter().any(|&v| v < 1.0); // part of the fiber off the image
    let mut shown = if partial { vec![0f32; radii.len() * bw * bh] } else { Vec::new() };
    let mut valid_pad = vec![0f32; if partial { pw * ph } else { 0 }];
    if partial {
        for y in 0..gx.h {
            valid_pad[(y + pad) * pw + pad..(y + pad) * pw + pad + gx.w].copy_from_slice(&valid.data[y * gx.w..(y + 1) * gx.w]);
        }
    }
    let mut across = vec![0f32; pw * ph];
    for a in linspace(0.0, PI, samples / 2, false) {
        let (c, s) = (a.cos() as f32, a.sin() as f32);
        for y in 0..gx.h {
            for x in 0..gx.w {
                across[(y + pad) * pw + x + pad] = (gx.at(x, y) * c + gy.at(x, y) * s).abs().min(cap);
            }
        }
        for (i, &r) in radii.iter().enumerate() {
            for sign in [1.0, -1.0] {
                let sx = (pad + x0) as isize + rne(sign * r * a.cos()) as isize;
                let sy = (pad + y0) as isize + rne(sign * r * a.sin()) as isize;
                let map = &mut maps[i * bw * bh..(i + 1) * bw * bh];
                for yy in 0..bh {
                    let src = &across[(sy as usize + yy) * pw + sx as usize..][..bw];
                    for (m, v) in map[yy * bw..(yy + 1) * bw].iter_mut().zip(src) {
                        *m += v;
                    }
                }
                if partial {
                    let cnt = &mut shown[i * bw * bh..(i + 1) * bw * bh];
                    for yy in 0..bh {
                        let src = &valid_pad[(sy as usize + yy) * pw + sx as usize..][..bw];
                        for (m, v) in cnt[yy * bw..(yy + 1) * bw].iter_mut().zip(src) {
                            *m += v;
                        }
                    }
                }
            }
        }
    }
    let n = samples as f32;
    if partial {
        for (m, c) in maps.iter_mut().zip(&shown) {
            *m /= c.max(MIN_SHOWN * n);
        }
    } else {
        maps.iter_mut().for_each(|v| *v /= n);
    }
    maps
}

/// Step 1 of find_rods: the best different rod pairs (and the best about the
/// center), and the quality (the best pair's score over a typical pair's).
fn coarse_pairs(gx: &ImgF, gy: &ImgF, valid: &ImgF, cap: f32, half: usize, n: usize, core: Option<(f64, f64)>, min_r: f64)
                -> (Vec<Pair>, f64) {
    let size = SYMMETRY_SIZE;
    let g = pair_grid(half);
    let maps = ring_maps(gx, gy, valid, cap, &g.sizes, 64, g.box_);
    let n_off = g.ox.len();
    let score: Vec<f32> = (0..g.inside.len())
        .map(|k| {
            let big_enough = g.sizes[g.radii[k / n_off]] >= min_r - 1e-9; // rods below the smallest size aren't tried
            if g.inside[k] && big_enough { maps[g.first[k]].min(maps[g.second[k]]) } else { 0.0 }
        })
        .collect();
    let centered = (0..n_off).min_by(|&a, &b| g.ox[a].hypot(g.oy[a]).partial_cmp(&g.ox[b].hypot(g.oy[b])).unwrap()).unwrap();
    let centred_scores: Vec<f32> = (0..g.angles.len())
        .filter(|&i| g.inside[i * n_off + centered])
        .map(|i| score[i * n_off + centered])
        .collect();
    let best = score.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let quality = best as f64 / (ip::median(&centred_scores) as f64).max(1e-6);
    let mut order: Vec<usize> = (0..score.len()).collect();
    let top = 2000.min(order.len());
    order.select_nth_unstable_by(top - 1, |&a, &b| score[b].partial_cmp(&score[a]).unwrap());
    order.truncate(top);
    order.sort_by(|&a, &b| score[b].partial_cmp(&score[a]).unwrap().then(a.cmp(&b)));
    let mut starts: Vec<(usize, usize)> = Vec::new();
    for k in order {
        let (i, o) = (k / n_off, k % n_off);
        let distinct = starts.iter().all(|&(j, q)| {
            (((g.angles[i] - g.angles[j]).to_degrees() + 90.0).rem_euclid(180.0) - 90.0).abs() > 4.0
                || (g.spacings[i] - g.spacings[j]).abs() > 0.05 * size
                || (g.sizes[g.radii[i]] - g.sizes[g.radii[j]]).abs() > 0.04 * size
                || (g.ox[o] - g.ox[q]).hypot(g.oy[o] - g.oy[q]) > 0.05 * size
        });
        if distinct {
            starts.push((i, o));
            if starts.len() == n {
                break;
            }
        }
    }
    // The best pair about the center too, and about the core if it shows.
    let mut anchors = vec![centered];
    if let Some((x, y)) = core {
        let (dx, dy) = (x - half as f64, y - half as f64);
        let o = (0..n_off).min_by(|&a, &b| (g.ox[a] - dx).hypot(g.oy[a] - dy).partial_cmp(&(g.ox[b] - dx).hypot(g.oy[b] - dy)).unwrap()).unwrap();
        anchors.push(o);
    }
    for o in anchors {
        if starts.iter().all(|&(_, q)| q != o) {
            let i = (0..g.angles.len()).fold(0, |b, i| if score[i * n_off + o] > score[b * n_off + o] { i } else { b });
            starts.push((i, o));
        }
    }
    let pairs = starts
        .into_iter()
        .map(|(i, o)| [half as f64 + g.ox[o], half as f64 + g.oy[o], g.angles[i], g.spacings[i], g.sizes[g.radii[i]]])
        .collect();
    (pairs, quality)
}

/// Step 2 of find_rods: each pair moved to where its outlines follow the edges best, in ever finer steps.
fn refine_pairs(gx: &ImgF, gy: &ImgF, valid: &ImgF, cap: f32, pairs: &[Pair], min_r: f64) -> Vec<Pair> {
    // Each pair on its own path, so all at once.
    std::thread::scope(|s| {
        let jobs: Vec<_> = pairs.iter().map(|&p| s.spawn(move || refine_pair(gx, gy, valid, cap, p, min_r))).collect();
        jobs.into_iter().map(|j| j.join().unwrap()).collect()
    })
}

fn refine_pair(gx: &ImgF, gy: &ImgF, valid: &ImgF, cap: f32, mut pair: Pair, min_r: f64) -> Pair {
    let mut steps = [2.0, 2.0, 1f64.to_radians(), 2.0, 2.0];
    for _ in 0..6 {
        for _ in 0..10 {
            let was = pair;
            for k in 0..5 {
                let mut best = (f32::NEG_INFINITY, pair);
                for d in -2..=2 {
                    let mut t = pair;
                    t[k] += d as f64 * steps[k];
                    t[4] = t[4].max(2.0).max(min_r); // no edges at all (a plain disc) can shrink a rod to nothing
                    let s = pair_score((gx, gy), valid, cap, &t, 128, true, true);
                    if s > best.0 {
                        best = (s, t);
                    }
                }
                pair = best.1;
            }
            if pair == was {
                break; // until nothing moves
            }
        }
        steps.iter_mut().for_each(|s| *s /= 2.0);
    }
    pair
}

/// Build the rod search grid now (it takes ~0.1 s, once per run), e.g. at
/// start-up, so the first measurement doesn't wait for it.
pub fn warm_up() {
    pair_grid((SYMMETRY_SIZE * 1.05) as usize);
}

/// Panda stress rods of the fiber: ([(x, y, r), (x, y, r)] in image pixels, quality, coverage).
pub fn find_rods(image: &RgbImage, cx: f64, cy: f64, radius: f64) -> (Vec<(f64, f64, f64)>, f64, f64) {
    let r = find_rods_in(&rod_gradients(image, cx, cy, radius), cx, cy, DEFAULT_MIN_ROD);
    (r.0, r.1, r.2)
}

/// min_rod: smallest rod radius tried, % of the fiber radius.
fn find_rods_in(g: &Grads, cx: f64, cy: f64, min_rod: f64) -> (Vec<(f64, f64, f64)>, f64, f64, Option<f64>) {
    let cap = EDGE_CAP * g.typical;
    // At the search scale; the default leaves the search exactly as it was (refining may go below 18%).
    let min_r = if min_rod > DEFAULT_MIN_ROD { min_rod.min(MAX_MIN_ROD) / 100.0 * SYMMETRY_SIZE } else { 0.0 };
    let clean_gx = ImgF { data: g.gx.data.iter().zip(&g.dirt.data).map(|(&v, &d)| if d { 0.0 } else { v }).collect(), ..g.gx.clone() };
    let clean_gy = ImgF { data: g.gy.data.iter().zip(&g.dirt.data).map(|(&v, &d)| if d { 0.0 } else { v }).collect(), ..g.gy.clone() };
    // Both coarse searches at once.
    let ((clean_starts, clean_q), (plain_starts, plain_q)) = std::thread::scope(|s| {
        let a = s.spawn(|| coarse_pairs(&clean_gx, &clean_gy, &g.valid, cap, g.half, ROD_STARTS, g.core, min_r));
        let b = coarse_pairs(&g.gx, &g.gy, &g.valid, cap, g.half, ROD_STARTS, g.core, min_r);
        (a.join().unwrap(), b)
    });
    let mut tried: Vec<Pair> = Vec::new();
    let mut qualities = Vec::new();
    for (starts, q) in [(clean_starts, clean_q), (plain_starts, plain_q)] {
        for p in starts {
            if !tried.contains(&p) {
                tried.push(p);
                qualities.push(q);
            }
        }
    }
    let refined = refine_pairs(&g.gx, &g.gy, &g.valid, cap, &tried, min_r);
    // Each pair's rank: how well its outlines follow edges, times how much of them shows.
    let mut ranked: Vec<(f32, f64, Pair, [(f64, f64); 2])> = Vec::new();
    for (p, &quality) in refined.iter().zip(&qualities) {
        let e = (pair_score((&g.gx, &g.gy), &g.valid, cap, p, 256, true, false)
            + pair_score((&clean_gx, &clean_gy), &g.valid, cap, p, 256, true, false)) / 2.0;
        let (dx, dy) = (p[3] * p[2].cos(), -p[3] * p[2].sin());
        let centers = [(p[0] + dx, p[1] + dy), (p[0] - dx, p[1] - dy)];
        let cov = centers.iter().map(|&(x, y)| edge_coverage(&g.gx, &g.gy, &g.valid, x, y, p[4], g.typical)).fold(f64::INFINITY, f64::min);
        ranked.push((e * cov as f32, quality, *p, centers));
    }
    // A visible core breaks near-ties: of the pairs within CORE_TIE of the best, the
    // rods sit either side of the core, so pairs centered away from it count less.
    // (Only near-ties: a speck of dirt taken for the core can't overrule a clear winner.)
    let top = ranked.iter().map(|r| r.0).fold(f32::NEG_INFINITY, f32::max);
    let weight = |r: &(f32, f64, Pair, [(f64, f64); 2])| -> f32 {
        match g.core {
            Some((kx, ky)) if r.0 >= CORE_TIE * top => {
                let off = (r.2[0] - kx).hypot(r.2[1] - ky) / (CORE_SPREAD * SYMMETRY_SIZE);
                r.0 * (-off * off).exp() as f32
            }
            Some(_) => 0.0,
            None => r.0,
        }
    };
    let mut best = ranked[0].clone();
    let mut best_w = weight(&best);
    for r in &ranked[1..] {
        let w = weight(r);
        if w > best_w {
            (best, best_w) = (r.clone(), w);
        }
    }
    let (_, quality, [mx, my, _, _, r], centers) = best;

    // 3. Each center to a fraction of a pixel.
    let mut fitted = Vec::new();
    let mut sizes = Vec::new();
    for (x0, y0) in centers {
        let (mut x, mut y, mut rr) = (x0, y0, r);
        for _ in 0..3 {
            match fit_rod_edge(&g.gx, &g.gy, x, y, rr, 0.12, 180) {
                Some(f) if (f.0 - x0).hypot(f.1 - y0) <= 0.1 * r && (f.2 - r).abs() <= 0.15 * r => (x, y, rr) = f,
                _ => break,
            }
        }
        fitted.push((x, y));
        sizes.push(rr);
    }
    let r = mean(&sizes).max(min_r);
    let mut shows: Vec<f64> = fitted.iter().map(|&(x, y)| edge_coverage(&g.gx, &g.gy, &g.valid, x, y, r, g.typical)).collect();
    let weak = if shows[1] < shows[0] { 1 } else { 0 };
    let strong = if shows[1] > shows[0] { 1 } else { 0 };
    let (shift, turned_gain) = match_rods(&g.gx, &g.gy, g.typical, &fitted, r, Some(&g.dirt), 16);
    let ((x0, y0), (x1, y1)) = (fitted[0], fitted[1]);
    if shows[weak] < 0.5 && 0.5 <= shows[strong] {
        let (sx, sy) = fitted[strong];
        fitted[weak] = (2.0 * mx - sx, 2.0 * my - sy);
    } else if let Some((sx, sy)) = shift {
        let diff = ((sy.atan2(sx) - (y1 - y0).atan2(x1 - x0)).to_degrees() + 90.0).rem_euclid(180.0) - 90.0;
        if diff.abs() <= MATCH_AGREE {
            let (px, py) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            fitted = vec![(px - sx / 2.0, py - sy / 2.0), (px + sx / 2.0, py + sy / 2.0)];
            shows = shows.iter().zip(&fitted).map(|(&was, &(x, y))| was.max(edge_coverage(&g.gx, &g.gy, &g.valid, x, y, r, g.typical))).collect();
        }
    }
    let rods = fitted
        .iter()
        .map(|&(x, y)| ((x - g.half as f64) / g.scale + cx, (y - g.half as f64) / g.scale + cy, r / g.scale))
        .collect();
    (rods, quality, shows.iter().cloned().fold(f64::INFINITY, f64::min), turned_gain)
}

/// Rod 0 to rod 1 as (dx, dy) at the search scale, by sliding rod 0's outline over rod 1;
/// and how much better it matches turned 180 degrees ("turned gain").
fn match_rods(gx: &ImgF, gy: &ImgF, typical: f32, centers: &[(f64, f64)], r: f64, dirt: Option<&Mask>, search: isize)
              -> (Option<(f64, f64)>, Option<f64>) {
    let strength = gx.zip(gy, |a, b| a.hypot(b).min(EDGE_CAP * typical));
    let ((ax, ay), (bx, by)) = (centers[0], centers[1]);
    if !(r > 0.0) || !centers.iter().all(|c| c.0.is_finite() && c.1.is_finite()) {
        return (None, None);
    }
    let size = (1.25 * r).ceil() as isize + 1;
    let (ai, aj, bi, bj) = (rne(ay) as isize, rne(ax) as isize, rne(by) as isize, rne(bx) as isize);
    let (h, w) = (strength.h as isize, strength.w as isize);
    if ai.min(aj).min(bi).min(bj) - size - search < 0 || ai.max(bi) + size + search >= h || aj.max(bj) + size + search >= w {
        return (None, None);
    }
    let n = (2 * size + 1) as usize;
    let template = strength.crop((aj - size) as usize, (ai - size) as usize, n, n);
    let an = (2 * (size + search) + 1) as usize;
    let area = strength.crop((bj - size - search) as usize, (bi - size - search) as usize, an, an);
    let (mx, my) = ((ax + bx) / 2.0, (ay + by) / 2.0);
    let ring = ImgF::from_fn(n, n, |x, y| {
        let (xx, yy) = (x as f64 - size as f64, y as f64 - size as f64);
        let d = xx.hypot(yy);
        let in_ring = d >= 0.75 * r && d <= 1.25 * r;
        // The core lies between the rods: left out.
        let off_core = (xx + aj as f64 - mx).hypot(yy + ai as f64 - my) > CORE_FRACTION * SYMMETRY_SIZE;
        if in_ring && off_core { 1.0 } else { 0.0 }
    });
    let mut used = ring.clone();
    if let Some(dirt) = dirt {
        let clean = ImgF::from_fn(n, n, |x, y| {
            let d = dirt.at((aj - size) as usize + x, (ai - size) as usize + y);
            if d { 0.0 } else { ring.at(x, y) }
        });
        if clean.data.iter().sum::<f32>() >= 0.5 * ring.data.iter().sum::<f32>() {
            used = clean;
        }
    }
    let flip = |m: &ImgF| ImgF::from_fn(m.w, m.h, |x, y| m.at(m.w - 1 - x, m.h - 1 - y));
    let matched = ip::match_template_ccorr_normed(&area, &template, &used);
    let turned = ip::match_template_ccorr_normed(&area, &flip(&template), &flip(&used));
    let (j, i) = matched.argmax();
    let best = matched.at(j, i);
    let gain = (turned.max_value() - best) as f64;
    if best < MATCH_SCORE {
        return (None, Some(gain));
    }
    let peak = |v: &[f32], k: usize| -> f64 {
        if k > 0 && k + 1 < v.len() {
            let (a, b, c) = (v[k - 1] as f64, v[k] as f64, v[k + 1] as f64);
            if a - 2.0 * b + c < 0.0 {
                return 0.5 * (a - c) / (a - 2.0 * b + c);
            }
        }
        0.0
    };
    let row: Vec<f32> = (0..matched.w).map(|x| matched.at(x, i)).collect();
    let col: Vec<f32> = (0..matched.h).map(|y| matched.at(j, y)).collect();
    let dx = (bj - aj) as f64 + j as f64 + peak(&row, j) - search as f64;
    let dy = (bi - ai) as f64 + i as f64 + peak(&col, i) - search as f64;
    (Some((dx, dy)), Some(gain))
}

/// Circle fitted to the strongest edge within spread x r of the circle along each ray from (x, y).
fn fit_rod_edge(gx: &ImgF, gy: &ImgF, x: f64, y: f64, r: f64, spread: f64, rays: usize) -> Option<(f64, f64, f64)> {
    let radii = linspace(r * (1.0 - spread), r * (1.0 + spread), 33, true);
    let mut all = Vec::with_capacity(rays * 33);
    let mut best = Vec::with_capacity(rays);
    for a in linspace(0.0, 2.0 * PI, rays, false) {
        let (ca, sa) = (a.cos(), a.sin());
        let mut top = (f32::NEG_INFINITY, 0.0, 0.0);
        for &rr in &radii {
            let (xs, ys) = ((x + ca * rr) as f32, (y + sa * rr) as f32);
            let v = (ip::sample(gx, xs, ys) * ca as f32 + ip::sample(gy, xs, ys) * sa as f32).abs();
            all.push(v);
            if v > top.0 {
                top = (v, xs as f64, ys as f64);
            }
        }
        best.push(top);
    }
    let med = ip::median(&all);
    let strong: Vec<_> = best.into_iter().filter(|b| b.0 > med).collect();
    if strong.len() < rays / 4 {
        return None;
    }
    let px: Vec<f64> = strong.iter().map(|b| b.1).collect();
    let py: Vec<f64> = strong.iter().map(|b| b.2).collect();
    fit_circle(&px, &py, 20)
}

/// Fraction of the circle's outline (in 32 arcs) with a clear edge right on it.
fn edge_coverage(gx: &ImgF, gy: &ImgF, valid: &ImgF, x: f64, y: f64, r: f64, typical: f32) -> f64 {
    let (n_arcs, per_arc) = (32, 8);
    let angles = linspace(0.0, 2.0 * PI, n_arcs * per_arc, false);
    let radii = linspace(0.75 * r, 1.25 * r, 41, true);
    let mut strongest = vec![0f32; n_arcs]; // over all radii
    let mut near = vec![0f32; n_arcs]; // over radii within 5% of r
    for &rr in &radii {
        let is_near = (rr - r).abs() <= 0.05 * r;
        for arc in 0..n_arcs {
            let mut m = 0f32;
            for k in 0..per_arc {
                let a = angles[arc * per_arc + k];
                let (ca, sa) = (a.cos(), a.sin());
                let (xs, ys) = ((x + rr * ca) as f32, (y + rr * sa) as f32);
                let v = (ip::sample(gx, xs, ys) * ca as f32 + ip::sample(gy, xs, ys) * sa as f32).abs();
                m = m.max(v);
            }
            strongest[arc] = strongest[arc].max(m);
            if is_near {
                near[arc] = near[arc].max(m);
            }
        }
    }
    // Arcs off the image don't count (but at least MIN_SHOWN of the arcs do).
    let shown = (0..n_arcs)
        .filter(|&arc| {
            let a = angles[arc * per_arc + per_arc / 2];
            ip::sample(valid, (x + r * a.cos()) as f32, (y + r * a.sin()) as f32) >= 0.5
        })
        .count() as f64;
    let good = (0..n_arcs).filter(|&a| near[a] >= ON_CIRCLE_SHARE * strongest[a] && near[a] > 2.0 * typical).count() as f64;
    good / shown.max(MIN_SHOWN as f64 * n_arcs as f64)
}

/// Mean capped gradient across an outline (points with unit normals).
fn boundary_fit(gx: &ImgF, gy: &ImgF, cap: f32, pts: &[(f64, f64, f64, f64)]) -> f64 {
    let s: f64 = pts.iter().map(|&(x, y, nx, ny)| {
        let (xs, ys) = (x as f32, y as f32);
        (ip::sample(gx, xs, ys) * nx as f32 + ip::sample(gy, xs, ys) * ny as f32).abs().min(cap) as f64
    }).sum();
    s / pts.len() as f64
}

/// How well rods (Panda), wedges (Bow-tie) and an ellipse (Elliptical) fit the edges, 0-1.
fn shape_fits(g: &Grads, cx: f64, cy: f64, rods: &[(f64, f64, f64)]) -> [(String, f64); 3] {
    let cap = EDGE_CAP * g.typical;
    let size = SYMMETRY_SIZE;
    let half = g.half as f64;
    let ((ax, ay, _), (bx, by, _)) = (rods[0], rods[1]);
    let theta = (-(by - ay)).atan2(bx - ax);
    let (hx, hy) = (((ax + bx) / 2.0 - cx) * g.scale + half, ((ay + by) / 2.0 - cy) * g.scale + half);
    let t = linspace(0.0, 2.0 * PI, 96, false);
    let mut pts = Vec::new();
    for &(x, y, r) in rods {
        for &a in &t {
            pts.push(((x - cx) * g.scale + half + r * g.scale * a.cos(), (y - cy) * g.scale + half + r * g.scale * a.sin(), a.cos(), a.sin()));
        }
    }
    let panda = boundary_fit(&g.gx, &g.gy, cap, &pts);

    let turns: Vec<f64> = [-6.0f64, -3.0, 0.0, 3.0, 6.0].iter().map(|d| d.to_radians()).collect();
    let s = linspace(-1.0, 1.0, 16, true);
    let u = linspace(0.0, 1.0, 16, true);
    let mut bowtie = f64::NEG_INFINITY;
    for &turn in &turns {
        for r1 in arange(0.03, 0.42, 0.03).into_iter().map(|v| v * size) {
            for r2 in arange(0.35, 0.9, 0.03).into_iter().map(|v| v * size) {
                if r2 <= r1 + 0.15 * size {
                    continue;
                }
                for half_angle in arange(8.0, 62.0, 3.0).into_iter().map(f64::to_radians) {
                    let mut pts = Vec::with_capacity(128);
                    for base in [0.0, PI] {
                        let b = theta + turn + base;
                        for r in [r1, r2] {
                            for &sv in &s {
                                let a = b + sv * half_angle;
                                pts.push((hx + r * a.cos(), hy - r * a.sin(), a.cos(), -a.sin()));
                            }
                        }
                        for side in [-1.0, 1.0] {
                            let a = b + side * half_angle;
                            for &uv in &u {
                                let rr = r1 + (r2 - r1) * uv;
                                pts.push((hx + rr * a.cos(), hy - rr * a.sin(), -a.sin(), -a.cos()));
                            }
                        }
                    }
                    bowtie = bowtie.max(boundary_fit(&g.gx, &g.gy, cap, &pts));
                }
            }
        }
    }

    let e = linspace(0.0, 2.0 * PI, 64, false);
    let mut ellipse = f64::NEG_INFINITY;
    for &turn in &turns {
        for across in [0.0, PI / 2.0] {
            for a_len in arange(0.25, 0.85, 0.03).into_iter().map(|v| v * size) {
                for b_len in arange(0.12, 0.6, 0.03).into_iter().map(|v| v * size) {
                    if b_len >= 0.95 * a_len {
                        continue;
                    }
                    let (ct, st) = ((theta + turn + across).cos(), (theta + turn + across).sin());
                    let pts: Vec<_> = e.iter().map(|&ee| {
                        let (pu, pv) = (a_len * ee.cos(), b_len * ee.sin());
                        let (nu, nv) = (ee.cos() / a_len, ee.sin() / b_len);
                        let (nx, ny) = (nu * ct - nv * st, -(nu * st + nv * ct));
                        let norm = nx.hypot(ny);
                        (hx + pu * ct - pv * st, hy - (pu * st + pv * ct), nx / norm, ny / norm)
                    }).collect();
                    ellipse = ellipse.max(boundary_fit(&g.gx, &g.gy, cap, &pts));
                }
            }
        }
    }
    let cap = cap as f64;
    [("Panda".into(), panda / cap), ("Bow-tie".into(), bowtie / cap), ("Elliptical".into(), ellipse / cap)]
}

fn measure_auto(image: &RgbImage, cx: f64, cy: f64, radius: f64, detection: &Detection) -> FiberAxis {
    let grads = rod_gradients(image, cx, cy, radius);
    let found = measure_rods(image, cx, cy, radius, Some(&grads), detection.min_rod_for(radius));
    if !found.clear() {
        // Washed-out rods: a clear apple core still shows the axes (the rods, faint as they are, say which is slow).
        if let Some(core) = apple_core(image, cx, cy, radius, rod_guide(&found)).filter(AppleCore::clear) {
            return core.axis(cx, cy, radius);
        }
    }
    if found.quality.unwrap_or(0.0) < OTHER_SHAPE_QUALITY {
        return found;
    }
    match found.turned_gain {
        Some(g) if g >= TURNED_GAIN => {}
        _ => return found,
    }
    let rods: Vec<_> = found.part_centres.iter().zip(&found.rod_radii).map(|(&(x, y), &r)| (x, y, r)).collect();
    let fits = shape_fits(&grads, cx, cy, &rods);
    let other = if fits[2].1 > fits[1].1 { &fits[2] } else { &fits[1] };
    if other.1 < fits[0].1 + OTHER_SHAPE_MARGIN {
        return found;
    }
    let mut result = measure_axis(image, cx, cy, radius, "symmetry", false, detection);
    result.fiber_type = other.0.clone();
    // The bright pattern round a Panda's core can fit a wedge or an ellipse better than
    // faint rods do: if the rods and the apple core agree, and the other shape doesn't, keep the rods.
    if let Some(core) = apple_core(image, cx, cy, radius, rod_guide(&found)).filter(AppleCore::clear) {
        if axial_diff(core.slow_deg, found.slow_deg) < APPLE_AGREE && axial_diff(core.slow_deg, result.slow_deg) >= APPLE_AGREE {
            return found;
        }
    }
    result
}

/// Difference of two axis angles (degrees, wrapping every 180°), 0 to 90.
fn axial_diff(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(180.0);
    d.min(180.0 - d)
}

/// The "apple core": the bright region round a Panda fiber's core, pinched on the two
/// sides where the rods press on it. Its two mirror axes are the fiber's axes, and its
/// long pinched sides face the rods, so the axis their edges point along is the slow one.
/// It often stays sharp when the rods are washed out (grey, front-lit pictures).
#[derive(Clone, Copy, Debug)]
pub struct AppleCore {
    pub slow_deg: f64,
    /// Its other mirror axis (90° from slow_deg): the fast axis.
    pub fast_deg: f64,
    /// Mirror correlation (-1 to 1) about the slow and the fast axis.
    pub symmetry: (f64, f64),
    /// How far the outline reaches along the slow and the fast axis, over its distance where
    /// the axis crosses it: 1 = straight or bulging side, more = pinched (concave).
    pub pinch: (f64, f64),
    /// The slow axis was picked by the rod outlines beside the core (not by the pinch).
    pub by_rods: bool,
}

impl AppleCore {
    pub fn clear(&self) -> bool {
        self.symmetry.0.max(self.symmetry.1) >= APPLE_SYMMETRY.0
            && self.symmetry.0.min(self.symmetry.1) >= APPLE_SYMMETRY.1
            && self.pinch.0 - self.pinch.1 >= APPLE_PINCH // pinched on one pair of sides: an apple core, not a round spot
    }

    fn axis(&self, cx: f64, cy: f64, radius: f64) -> FiberAxis {
        let mut f = FiberAxis::new(cx, cy, radius, self.slow_deg, 1.0, if self.clear() { "Panda" } else { "?" }, 0);
        f.method = "core".into();
        f.core_clear = Some(self.clear());
        f
    }
}

/// The rod outlines' angle, if they're worth using to orient the apple core.
fn rod_guide(rods: &FiberAxis) -> Option<f64> {
    (rods.quality.unwrap_or(0.0) >= APPLE_ROD_QUALITY).then_some(rods.slow_deg)
}

/// The apple core round (cx, cy), or None if that part of the fiber is off the image.
/// `rods_deg`: the line through the rod outlines found beside it, even faint ones: of the
/// core's two mirror axes, the one along it is the slow axis (the rods sit beside the
/// pinches). Without it, or with it far from both axes, the pinches decide.
pub fn apple_core(image: &RgbImage, cx: f64, cy: f64, radius: f64, rods_deg: Option<f64>) -> Option<AppleCore> {
    let reach = radius * APPLE_REACH;
    if cx - reach < 0.0 || cy - reach < 0.0 || cx + reach > image.width() as f64 || cy + reach > image.height() as f64 || reach < 4.0 {
        return None;
    }
    let gray = ImgF::from_gray(&ip::to_gray(image));
    let k = (reach.min(APPLE_SIZE)) as usize; // half-size of the copy
    let step = reach / k as f64;
    let n = 2 * k;
    let mut crop = ImgF::new(n, n);
    for y in 0..n {
        for x in 0..n {
            let (u, v) = (cx + (x as f64 - k as f64 + 0.5) * step, cy + (y as f64 - k as f64 + 0.5) * step);
            crop.data[y * n + x] = ip::sample(&gray, u as f32 - 0.5, v as f32 - 0.5);
        }
    }
    let crop = ip::gaussian_blur(&crop, k as f64 / 60.0);
    let half = k as f64;
    let disc: Vec<(usize, usize)> = (0..n * n)
        .map(|i| (i % n, i / n))
        .filter(|&(x, y)| (x as f64 + 0.5 - half).hypot(y as f64 + 0.5 - half) < 0.95 * half)
        .collect();
    // Pearson correlation of the copy with its mirror image about the line at `deg`.
    let mirror = |deg: f64| -> f64 {
        let t = deg.to_radians();
        let (ux, uy) = (t.cos(), -t.sin()); // y down
        let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for &(x, y) in &disc {
            let (px, py) = (x as f64 + 0.5 - half, y as f64 + 0.5 - half);
            let d = px * ux + py * uy;
            let (rx, ry) = (2.0 * d * ux - px, 2.0 * d * uy - py);
            let a = crop.data[y * n + x] as f64;
            let b = ip::sample(&crop, (rx + half - 0.5) as f32, (ry + half - 0.5) as f32) as f64;
            sa += a;
            sb += b;
            saa += a * a;
            sbb += b * b;
            sab += a * b;
        }
        let m = disc.len() as f64;
        let cov = sab / m - sa * sb / (m * m);
        let var = ((saa / m - (sa / m).powi(2)) * (sbb / m - (sb / m).powi(2))).sqrt();
        if var > 1e-9 { cov / var } else { 0.0 }
    };
    // Both axes at once: the best pair 90° apart, to 1° and then 0.1°.
    let pair = |deg: f64| mirror(deg) + mirror(deg + 90.0);
    let best = |angles: Vec<f64>| -> f64 {
        let scores: Vec<f64> = std::thread::scope(|s| {
            let jobs: Vec<_> = angles.chunks(angles.len().div_ceil(8)).map(|c| s.spawn(|| c.iter().map(|&a| pair(a)).collect::<Vec<_>>())).collect();
            jobs.into_iter().flat_map(|j| j.join().unwrap()).collect()
        });
        let i = (0..scores.len()).max_by(|&a, &b| scores[a].partial_cmp(&scores[b]).unwrap()).unwrap();
        angles[i]
    };
    let coarse = best(arange(0.0, 90.0, 1.0));
    let theta = best(arange(coarse - 1.0, coarse + 1.001, 0.1)).rem_euclid(90.0);
    // Which axis goes through the pinches: trace the outline (the strongest edge along rays
    // from the center) and see how far it reaches along each axis. A pinched side's corners
    // reach further than its middle; a straight or bulging side's middle reaches furthest.
    let ray_edge = |deg: f64| -> f64 {
        let t = deg.to_radians();
        let (ux, uy) = (t.cos(), -t.sin());
        let at = |r: f64| ip::sample(&crop, (half + r * ux - 0.5) as f32, (half + r * uy - 0.5) as f32) as f64;
        let mut best = (0.0, half);
        let mut r = 0.1 * half;
        while r < 0.93 * half {
            let g = (at(r + 1.5) - at(r - 1.5)).abs();
            if g > best.0 {
                best = (g, r);
            }
            r += 0.5;
        }
        best.1
    };
    let pinch = |axis: f64| -> f64 {
        let mut total = 0.0;
        for end in [0.0, 180.0] {
            let middle = [-2.0, 0.0, 2.0].iter().map(|d| ray_edge(axis + end + d)).sum::<f64>() / 3.0;
            let reach = (1..=APPLE_SIDE as usize)
                .flat_map(|d| [d as f64, -(d as f64)])
                .map(|d| ray_edge(axis + end + d) * d.to_radians().cos())
                .fold(middle, f64::max);
            total += reach / middle.max(1.0);
        }
        total / 2.0
    };
    let (a, b) = (pinch(theta), pinch(theta + 90.0));
    let by_pinch = if a >= b { (theta, theta + 90.0) } else { (theta + 90.0, theta) };
    let guide = rods_deg.filter(|&r| axial_diff(r, theta).min(axial_diff(r, theta + 90.0)) <= APPLE_ROD_GUIDE);
    let (slow, other) = match guide {
        Some(r) if axial_diff(r, theta) <= axial_diff(r, theta + 90.0) => (theta, theta + 90.0),
        Some(_) => (theta + 90.0, theta),
        None => by_pinch,
    };
    Some(AppleCore {
        slow_deg: slow.rem_euclid(180.0),
        fast_deg: other.rem_euclid(180.0),
        symmetry: (mirror(slow), mirror(other)),
        pinch: (a.max(b), a.min(b)),
        by_rods: guide.is_some(),
    })
}

fn measure_rods(image: &RgbImage, cx: f64, cy: f64, radius: f64, grads: Option<&Grads>, min_rod: f64) -> FiberAxis {
    let own;
    let g = match grads {
        Some(g) => g,
        None => {
            own = rod_gradients(image, cx, cy, radius);
            &own
        }
    };
    let (rods, quality, coverage, turned_gain) = find_rods_in(g, cx, cy, min_rod);
    let ((ax, ay, ar), (bx, by, br)) = (rods[0], rods[1]);
    let angle = (-(by - ay)).atan2(bx - ax).to_degrees().rem_euclid(180.0);
    let (half_gap, rod) = ((bx - ax).hypot(by - ay) / 2.0, (ar + br) / 2.0);
    let elongation = (1.0 + 4.0 * half_gap * half_gap / (rod * rod)).sqrt();
    let a = linspace(0.0, 2.0 * PI, 72, false);
    let outlines = rods.iter().map(|&(x, y, r)| a.iter().map(|&t| (x + r * t.cos(), y + r * t.sin())).collect()).collect();
    let mut found = FiberAxis::new(cx, cy, radius, angle, elongation, "?", 2);
    found.method = "rods".into();
    found.axis_x = Some((ax + bx) / 2.0);
    found.axis_y = Some((ay + by) / 2.0);
    found.parts = outlines;
    found.part_centres = vec![(ax, ay), (bx, by)];
    found.quality = Some(quality);
    found.coverage = Some(coverage);
    found.rod_radii = vec![ar, br];
    found.turned_gain = turned_gain;
    if found.clear() {
        found.fiber_type = "Panda".into();
    }
    found
}

/// Otsu's threshold of float values (the top 0.1% clipped off).
fn otsu_f(values: &[f32]) -> f64 {
    let top = ip::percentile(values, 99.9) as f64;
    if top <= 0.0 {
        return 0.0;
    }
    let mut hist = [0f64; 256];
    for &v in values {
        let v = (v as f64).clamp(0.0, top);
        let k = ((v / top) * 256.0) as usize;
        hist[k.min(255)] += 1.0;
    }
    let total: f64 = hist.iter().sum();
    let edges: Vec<f64> = (0..=256).map(|i| top * i as f64 / 256.0).collect();
    let (mut w0, mut m0) = (0.0, 0.0);
    let mut cum = Vec::with_capacity(256);
    for i in 0..256 {
        let p = hist[i] / total;
        w0 += p;
        m0 += p * (edges[i] + edges[i + 1]) / 2.0;
        cum.push((w0, m0));
    }
    let m_total = cum[255].1;
    let mut best = (f64::NEG_INFINITY, 0);
    for (i, &(w, m)) in cum[..255].iter().enumerate() {
        let between = (m_total * w - m).powi(2) / (w * (1.0 - w)).max(1e-12);
        if between > best.0 {
            best = (between, i);
        }
    }
    edges[best.1 + 1]
}

/// The slow axis of the fiber with cladding circle (cx, cy, radius).
pub fn measure_axis(image: &RgbImage, cx: f64, cy: f64, radius: f64, method: &str, parts_centre: bool, detection: &Detection) -> FiberAxis {
    if method == "auto" {
        return measure_auto(image, cx, cy, radius, detection);
    }
    if method == "rods" {
        return measure_rods(image, cx, cy, radius, None, detection.min_rod_for(radius));
    }
    if method == "core" {
        let rods = measure_rods(image, cx, cy, radius, None, detection.min_rod_for(radius));
        return match apple_core(image, cx, cy, radius, rod_guide(&rods)) {
            Some(core) => core.axis(cx, cy, radius),
            None => FiberAxis::new(cx, cy, radius, 0.0, 1.0, "?", 0),
        };
    }
    let (w, h) = (image.width() as i64, image.height() as i64);
    let x0 = ((cx - radius) as i64).max(0);
    let x1 = ((cx + radius) as i64 + 1).min(w);
    let y0 = ((cy - radius) as i64).max(0);
    let y1 = ((cy + radius) as i64 + 1).min(h);
    if x1 <= x0 || y1 <= y0 {
        return FiberAxis::new(cx, cy, radius, 0.0, 1.0, "?", 0);
    }
    let crop = image::imageops::crop_imm(image, x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32).to_image();
    let (ox, oy) = (x0 as f64, y0 as f64);
    let (diff, ring) = contrast_map(&crop, cx - ox, cy - oy, radius, detection);
    if ring.count() < 50 {
        return FiberAxis::new(cx, cy, radius, 0.0, 1.0, "?", 0);
    }
    let threshold = detection.threshold.unwrap_or_else(|| {
        let values: Vec<f32> = (0..diff.data.len()).filter(|&i| ring.data[i]).map(|i| diff.data[i]).collect();
        otsu_f(&values).max(MIN_AUTO_THRESHOLD)
    });
    let mask = Mask::from_fn(diff.w, diff.h, |x, y| ring.at(x, y) && diff.at(x, y) as f64 > threshold);

    // Principal axis of the stress parts about the fiber center.
    let (mut wx, mut wy) = (Vec::new(), Vec::new());
    for y in 0..mask.h {
        for x in 0..mask.w {
            if mask.at(x, y) {
                wx.push(x as f64 + ox - cx);
                wy.push(-(y as f64 + oy - cy)); // y flipped: angles as seen on screen
            }
        }
    }
    let (mut px, mut py) = (cx, cy);
    if parts_centre && !wx.is_empty() {
        let (mx, my) = (mean(&wx), mean(&wy));
        px = cx + mx;
        py = cy - my;
        wx.iter_mut().for_each(|v| *v -= mx);
        wy.iter_mut().for_each(|v| *v -= my);
    }
    if wx.len() < 20 {
        let mut f = FiberAxis::new(cx, cy, radius, 0.0, 1.0, "?", 0);
        f.threshold = threshold;
        return f;
    }
    let n = wx.len() as f64;
    let mu20 = wx.iter().map(|v| v * v).sum::<f64>() / n;
    let mu02 = wy.iter().map(|v| v * v).sum::<f64>() / n;
    let mu11 = wx.iter().zip(&wy).map(|(a, b)| a * b).sum::<f64>() / n;
    let mut angle = (0.5 * (2.0 * mu11).atan2(mu20 - mu02).to_degrees()).rem_euclid(180.0);
    let spread = (((mu20 - mu02) / 2.0).powi(2) + mu11 * mu11).sqrt();
    let (along, across) = ((mu20 + mu02) / 2.0 + spread, (mu20 + mu02) / 2.0 - spread);
    let elongation = (along / across.max(1e-9)).sqrt();

    let parts = stress_parts(&mask, radius * detection.min_size / 100.0);
    let (n_parts, fiber_type) = classify(&parts);
    let mut used = "moments";
    let centroids: Vec<(f64, f64)> = parts.iter().map(|c| ip::contour_centroid(c)).collect();
    if method == "centres" && parts.len() == 2 {
        let ((ax, ay), (bx, by)) = (centroids[0], centroids[1]);
        angle = (-(by - ay)).atan2(bx - ax).to_degrees().rem_euclid(180.0);
        used = "centres";
    } else if method == "symmetry" {
        angle = symmetry_angle(&diff, px - ox, py - oy, radius, angle);
        used = "symmetry";
    }
    let (axis_x, axis_y) = if used == "centres" {
        let ((ax, ay), (bx, by)) = (centroids[0], centroids[1]);
        (Some(ox + (ax + bx) / 2.0), Some(oy + (ay + by) / 2.0))
    } else if parts_centre {
        (Some(px), Some(py))
    } else {
        (None, None)
    };
    let outlines = parts
        .iter()
        .map(|c| ip::approx_poly(c, (radius * 0.005).max(0.5)).into_iter().map(|(x, y)| (x + ox, y + oy)).collect())
        .collect();
    let mut f = FiberAxis::new(cx, cy, radius, angle, elongation, &fiber_type, n_parts);
    f.method = used.into();
    f.axis_x = axis_x;
    f.axis_y = axis_y;
    f.parts = outlines;
    f.part_centres = centroids.iter().map(|&(x, y)| (x + ox, y + oy)).collect();
    f.threshold = threshold;
    f
}

/// Outlines of the stress parts, largest first, specks left out.
fn stress_parts(mask: &Mask, min_side: f64) -> Vec<Vec<(f64, f64)>> {
    let opened = ip::open(mask, &ip::rect_kernel(3, 3));
    let mut parts: Vec<(f64, Vec<(f64, f64)>)> = ip::outer_contours(&opened)
        .into_iter()
        .map(|c| ip::simplify_chain(&c))
        .map(|c| (ip::contour_area(&c), c))
        .filter(|(a, _)| *a >= min_side * min_side)
        .collect();
    parts.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    parts.into_iter().map(|p| p.1).collect()
}

fn classify(parts: &[Vec<(f64, f64)>]) -> (usize, String) {
    match parts.len() {
        1 => (1, "Elliptical".into()),
        2 => {
            let roundness = parts
                .iter()
                .map(|c| 4.0 * PI * ip::contour_area(c) / ip::arc_length(c).powi(2).max(1e-9))
                .fold(f64::INFINITY, f64::min);
            (2, if roundness > 0.8 { "Panda" } else { "Bow-tie" }.into())
        }
        n => (n, "?".into()),
    }
}

/// The axis angle the map is most mirror-symmetric about (with the line at 90 degrees), near start_deg.
fn symmetry_angle(diff: &ImgF, cx: f64, cy: f64, radius: f64, start_deg: f64) -> f64 {
    let scale = (SYMMETRY_SIZE / radius).min(1.0);
    let half = (radius * EDGE_FRACTION * scale) as isize;
    let size = (2 * half + 1) as usize;
    let disk: Vec<usize> = (0..size * size)
        .filter(|&i| {
            let (x, y) = ((i % size) as f64 - half as f64, (i / size) as f64 - half as f64);
            let d = x.hypot(y);
            d < half as f64 && d > radius * scale * CORE_FRACTION
        })
        .collect();
    let error = |deg: f64| -> f64 {
        // getRotationMatrix2D((cx, cy), -deg, scale), then moved so the center is the middle pixel.
        let a = (-deg).to_radians();
        let (al, be) = (scale * a.cos(), scale * a.sin());
        let m: ip::Affine = [
            [al, be, (1.0 - al) * cx - be * cy + half as f64 - cx],
            [-be, al, be * cx + (1.0 - al) * cy + half as f64 - cy],
        ];
        let rot = ip::warp_affine(diff, &m, size, size, false, false);
        let mut total = 0.0;
        for &i in &disk {
            let (x, y) = (i % size, i / size);
            let v = rot.data[i];
            total += ((v - rot.at(x, size - 1 - y)).abs() + (v - rot.at(size - 1 - x, y)).abs()) as f64;
        }
        total / disk.len().max(1) as f64
    };
    let argmin = |values: Vec<f64>| -> f64 {
        // Every angle at once (each error is independent), then the first smallest.
        let errors: Vec<f64> = std::thread::scope(|s| {
            let chunks: Vec<_> = values
                .chunks(values.len().div_ceil(8))
                .map(|c| s.spawn(|| c.iter().map(|&v| error(v)).collect::<Vec<_>>()))
                .collect();
            chunks.into_iter().flat_map(|h| h.join().unwrap()).collect()
        });
        let mut best = (f64::INFINITY, values[0]);
        for (&v, &e) in values.iter().zip(&errors) {
            if e < best.0 {
                best = (e, v);
            }
        }
        best.1
    };
    let best = argmin(arange(start_deg - 20.0, start_deg + 20.01, 1.0));
    let best = argmin(arange(best - 1.0, best + 1.001, 0.05));
    best.rem_euclid(180.0)
}

/// Axes of the given fibers, or of every fiber found in the image.
pub fn analyse(image: &RgbImage, circles: Option<&[(f64, f64, f64)]>, method: &str, parts_centre: bool,
               detection: &Detection, centre_only: bool) -> Vec<FiberAxis> {
    analyse_tracking(image, circles, None, method, parts_centre, detection, centre_only)
}

/// How far a followed fiber may move (center, radius) between updates, fractions of its radius.
const TRACK_MOVE: f64 = 0.1;
const TRACK_SIZE: f64 = 0.05;

/// Whether two sets of circles are the same fibers (each one within the
/// distance a followed fiber may move). Live updates only start following
/// fibers once two searches in a row agree, so one wrong search isn't followed.
pub fn same_fibers(a: &[(f64, f64, f64)], b: &[(f64, f64, f64)]) -> bool {
    !a.is_empty()
        && a.len() == b.len()
        && b.iter().all(|&(x, y, r)| {
            a.iter().any(|&(ax, ay, ar)| (x - ax).hypot(y - ay) < TRACK_MOVE * ar && (r - ar).abs() < TRACK_SIZE * ar)
        })
}

/// The fibers found last time, followed into this image: each circle re-fitted to
/// its cladding edge. None if any has moved much or can't be fitted (search again).
pub fn track_fibers(image: &RgbImage, previous: &[(f64, f64, f64)]) -> Option<Vec<(f64, f64, f64)>> {
    let gray = ip::to_gray(image);
    std::thread::scope(|s| {
        let jobs: Vec<_> = previous
            .iter()
            .map(|&c| {
                let gray = &gray;
                s.spawn(move || {
                    let f = fit_cladding_edge(gray, c.0, c.1, c.2, 360)?;
                    ((f.0 - c.0).hypot(f.1 - c.1) < TRACK_MOVE * c.2 && (f.2 - c.2).abs() < TRACK_SIZE * c.2).then_some(f)
                })
            })
            .collect();
        jobs.into_iter().map(|j| j.join().unwrap()).collect()
    })
}

/// As analyse; with `previous` (the fibers measured on the last live frame) they
/// are followed instead of searched for, which is much faster, if they still fit.
pub fn analyse_tracking(image: &RgbImage, circles: Option<&[(f64, f64, f64)]>, previous: Option<&[(f64, f64, f64)]>,
                        method: &str, parts_centre: bool, detection: &Detection, centre_only: bool) -> Vec<FiberAxis> {
    analyse_in(image, circles, previous, method, parts_centre, detection, centre_only, None)
}

/// The most fibers looked for in one image.
pub const MAX_FIBERS: usize = 50;

/// Where fibers are looked for (a region of interest), in image pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "lowercase")]
pub enum Region {
    Rect { x0: f64, y0: f64, x1: f64, y1: f64 },
    Circle { cx: f64, cy: f64, r: f64 },
}

impl Region {
    /// The same region moved by (dx, dy).
    pub fn moved(&self, dx: f64, dy: f64) -> Self {
        match *self {
            Region::Rect { x0, y0, x1, y1 } => Region::Rect { x0: x0 + dx, y0: y0 + dy, x1: x1 + dx, y1: y1 + dy },
            Region::Circle { cx, cy, r } => Region::Circle { cx: cx + dx, cy: cy + dy, r },
        }
    }

    /// The rectangle with these two corners.
    pub fn rect(a: (f64, f64), b: (f64, f64)) -> Self {
        Region::Rect { x0: a.0.min(b.0), y0: a.1.min(b.1), x1: a.0.max(b.0), y1: a.1.max(b.1) }
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        match *self {
            Region::Rect { x0, y0, x1, y1 } => x >= x0 && x <= x1 && y >= y0 && y <= y1,
            Region::Circle { cx, cy, r } => (x - cx).hypot(y - cy) <= r,
        }
    }

    /// (x0, y0, x1, y1)
    pub fn bounds(&self) -> (f64, f64, f64, f64) {
        match *self {
            Region::Rect { x0, y0, x1, y1 } => (x0, y0, x1, y1),
            Region::Circle { cx, cy, r } => (cx - r, cy - r, cx + r, cy + r),
        }
    }
}

/// Fibers whose centers are in the region: searched for in just that part of the image.
pub fn find_in_region(image: &RgbImage, region: &Region) -> Vec<(f64, f64, f64)> {
    let (bx0, by0, bx1, by1) = region.bounds();
    let x0 = bx0.max(0.0).floor() as u32;
    let y0 = by0.max(0.0).floor() as u32;
    let x1 = (bx1.ceil().max(0.0) as u32).min(image.width());
    let y1 = (by1.ceil().max(0.0) as u32).min(image.height());
    if x1 < x0 + 16 || y1 < y0 + 16 {
        return Vec::new();
    }
    let crop = image::imageops::crop_imm(image, x0, y0, x1 - x0, y1 - y0).to_image();
    find_fibers(&crop, MAX_FIBERS).into_iter().map(|(x, y, r)| (x + x0 as f64, y + y0 as f64, r)).filter(|c| region.contains(c.0, c.1)).collect()
}

/// As analyse_tracking; with a region, fibers are only looked for (and kept) inside it.
#[allow(clippy::too_many_arguments)]
pub fn analyse_in(image: &RgbImage, circles: Option<&[(f64, f64, f64)]>, previous: Option<&[(f64, f64, f64)]>,
                  method: &str, parts_centre: bool, detection: &Detection, centre_only: bool, region: Option<&Region>) -> Vec<FiberAxis> {
    let inside = |c: &(f64, f64, f64)| region.is_none_or(|r| r.contains(c.0, c.1));
    let circles: Vec<(f64, f64, f64)> = match (circles, previous) {
        (Some(c), _) if !c.is_empty() => c.to_vec(),
        (_, Some(prev)) if !prev.is_empty() => match track_fibers(image, prev) {
            Some(c) => c.into_iter().filter(inside).collect(),
            None => return analyse_in(image, None, None, method, parts_centre, detection, centre_only, region),
        },
        _ => {
            let found = match region {
                Some(r) => find_in_region(image, r),
                None => find_fibers(image, MAX_FIBERS),
            };
            match (centre_only, region) {
                (false, _) => found,
                (true, None) => nearest_centre(&found, image.width(), image.height()),
                (true, Some(r)) => {
                    let (x0, y0, x1, y1) = r.bounds();
                    let moved: Vec<_> = found.iter().map(|c| (c.0 - x0, c.1 - y0, c.2)).collect();
                    nearest_centre(&moved, (x1 - x0) as u32, (y1 - y0) as u32).into_iter().map(|c| (c.0 + x0, c.1 + y0, c.2)).collect()
                }
            }
        }
    };
    // Each fiber on its own thread (V-groove images hold several).
    std::thread::scope(|s| {
        let jobs: Vec<_> = circles
            .iter()
            .enumerate()
            .map(|(i, &(x, y, r))| {
                let others: Vec<(f64, f64, f64)> = circles.iter().enumerate().filter(|&(j, _)| j != i).map(|(_, &c)| c).collect();
                s.spawn(move || {
                    let mut f = measure_axis(image, x, y, r, method, parts_centre, detection);
                    f.centre_brightness = Some(centre_brightness(image, x, y, r));
                    f.image_quality = fiber_quality(image, f.cx, f.cy, f.radius, &others);
                    if f.rod_radii.len() == 2 && f.part_centres.len() == 2 {
                        let rods: Vec<(f64, f64, f64)> = f.part_centres.iter().zip(&f.rod_radii).map(|(&(x, y), &r)| (x, y, r)).collect();
                        f.rod_edge = rod_edge_width(image, &rods);
                        f.rod_contrast = rod_contrast(image, (f.cx, f.cy, f.radius), &rods, CORE_FRACTION * f.radius);
                    }
                    f
                })
            })
            .collect();
        jobs.into_iter().map(|j| j.join().unwrap()).collect()
    })
}

/// Brightness (luma, 0-255) of the brightest spot within CORE_SEARCH of the
/// fiber center: the peak of a small box average (2% of the radius, at least
/// 3x3 px), so one hot pixel doesn't count. Light sent into a fiber shows here.
pub fn centre_brightness(image: &RgbImage, cx: f64, cy: f64, radius: f64) -> f64 {
    let (w, h) = (image.width() as i64, image.height() as i64);
    let reach = (CORE_SEARCH * radius).max(2.0);
    let k = ((0.02 * radius).round() as i64).max(1);
    let (x0, x1) = (((cx - reach) as i64 - k).max(0), ((cx + reach).ceil() as i64 + k + 1).min(w));
    let (y0, y1) = (((cy - reach) as i64 - k).max(0), ((cy + reach).ceil() as i64 + k + 1).min(h));
    if x1 <= x0 || y1 <= y0 {
        return 0.0;
    }
    let (cw, ch) = ((x1 - x0) as usize, (y1 - y0) as usize);
    // Summed-area table of the luma.
    let mut sat = vec![0f64; (cw + 1) * (ch + 1)];
    for y in 0..ch {
        let mut row = 0.0;
        for x in 0..cw {
            let p = image.get_pixel((x0 + x as i64) as u32, (y0 + y as i64) as u32).0;
            row += 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
            sat[(y + 1) * (cw + 1) + x + 1] = sat[y * (cw + 1) + x + 1] + row;
        }
    }
    let mut best = f64::NEG_INFINITY;
    for y in 0..ch as i64 {
        for x in 0..cw as i64 {
            let (px, py) = ((x0 + x) as f64 + 0.5, (y0 + y) as f64 + 0.5);
            if (px - cx).hypot(py - cy) > reach {
                continue;
            }
            let (ax, bx) = ((x - k).max(0) as usize, (x + k + 1).min(cw as i64) as usize);
            let (ay, by) = ((y - k).max(0) as usize, (y + k + 1).min(ch as i64) as usize);
            let sum = sat[by * (cw + 1) + bx] - sat[ay * (cw + 1) + bx] - sat[by * (cw + 1) + ax] + sat[ay * (cw + 1) + ax];
            best = best.max(sum / ((bx - ax) * (by - ay)) as f64);
        }
    }
    if best.is_finite() { best } else { 0.0 }
}

/// Of two or more fibers, the one with the brightest center: (its index, its
/// position counted from the right, 1 = rightmost). None for fewer than two.
pub fn brightest_centre(axes: &[FiberAxis]) -> Option<(usize, usize)> {
    if axes.len() < 2 {
        return None;
    }
    let (i, f) = axes
        .iter()
        .enumerate()
        .filter(|(_, f)| f.centre_brightness.is_some())
        .max_by(|a, b| a.1.centre_brightness.partial_cmp(&b.1.centre_brightness).unwrap())?;
    let position = 1 + axes.iter().filter(|o| o.cx > f.cx).count();
    Some((i, position))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw;

    fn angle_error(measured: f64, truth: f64) -> f64 {
        (((measured - truth) + 90.0).rem_euclid(180.0) - 90.0).abs()
    }

    /// A drawn Panda fiber at a known angle (the Python self-test's first case).
    fn drawn_panda(angle: f64) -> RgbImage {
        let mut img = RgbImage::from_pixel(400, 400, image::Rgb([30, 30, 30]));
        // Python draws in BGR: (240, 220, 180) etc.; here as RGB.
        draw::fill_circle_px(&mut img, 200, 200, 150, [180, 220, 240]);
        let a = angle.to_radians();
        for s in [-1.0, 1.0] {
            draw::fill_circle_px(&mut img, rne(200.0 + s * 75.0 * a.cos()) as i64, rne(200.0 - s * 75.0 * a.sin()) as i64, 35, [30, 120, 200]);
        }
        draw::fill_circle_px(&mut img, 200, 200, 8, [240, 220, 60]);
        img
    }

    #[test]
    fn drawn_panda_every_method() {
        for truth in [0.0, 30.0, 75.0, 120.0] {
            let img = drawn_panda(truth);
            for &(method, _) in METHODS.iter().filter(|m| m.0 != "core") {
                let f = measure_axis(&img, 200.0, 200.0, 150.0, method, false, &Detection::default());
                let used = if method == "auto" { "rods" } else { method };
                assert!(angle_error(f.slow_deg, truth) < 2.0 && f.fiber_type == "Panda" && f.method == used,
                        "{truth} {method}: {} {} {}", f.slow_deg, f.fiber_type, f.method);
            }
        }
    }

    /// Fibers side by side, the outer rods running off the image, one rod under a dark
    /// blotch: rods measured over the part shown, and a visible core breaking near-ties.
    #[test]
    fn real_vgroove_photo() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../WIN_20261008_07_31_07_Pro.jpg");
        let Some(img) = crate::capture::load_image(&path) else { return }; // the user's photo; not in the repository
        let mut found = analyse(&img, None, "auto", false, &Detection::default(), false);
        found.sort_by(|a, b| a.cx.partial_cmp(&b.cx).unwrap());
        assert_eq!(found.len(), 3);
        // Angles judged from the rod outlines by eye: left -2, middle -0.9, right about -0.3.
        for (f, truth) in found.iter().zip([-2.0, -0.9, -0.3]) {
            assert!(f.fiber_type == "Panda" && f.clear(), "{f:?}");
            assert!(angle_error(f.slow_deg, truth) < 1.0, "{} vs {truth}", f.slow_deg);
        }
    }

    #[test]
    fn rod_running_off_the_image() {
        for truth in [0.0f64, 8.0, 172.0] {
            let mut img = RgbImage::from_pixel(800, 500, image::Rgb([12, 12, 12]));
            draw::fill_circle_px(&mut img, 690, 250, 175, [200, 200, 200]);
            let a: f64 = truth.to_radians();
            for s in [-1.0, 1.0] {
                let (x, y) = (690.0 + s * 95.0 * a.cos(), 250.0 - s * 95.0 * a.sin());
                draw::ring_px(&mut img, rne(x) as i64, rne(y) as i64, 48, 3, [150, 150, 150]);
            }
            let img = crate::imgproc::gaussian_blur_rgb(&img, 1.5);
            let f = measure_axis(&img, 690.0, 250.0, 175.0, "rods", false, &Detection::default());
            assert!(angle_error(f.slow_deg, truth) < 1.0, "{truth}: {}", f.slow_deg);
            let right = f.part_centres.iter().map(|c| c.0).fold(f64::NEG_INFINITY, f64::max);
            assert!((right - (690.0 + 95.0 * a.cos().abs())).abs() < 4.0, "{truth}: right rod at {right}");
        }
    }

    /// No fiber in view, only the curved edge of a bright area on a grey field (the
    /// user's 2026-10-27 video): nothing found.
    #[test]
    fn no_fiber_on_a_lit_edge() {
        let img = RgbImage::from_fn(960, 540, |x, y| {
            let lit = (x as f64 - 1500.0).hypot(y as f64 - 270.0) < 880.0;
            let v = if lit { 255.0 } else { 140.0 + 30.0 * x as f64 / 960.0 };
            image::Rgb([v as u8; 3])
        });
        let img = crate::imgproc::gaussian_blur_rgb(&img, 5.0);
        assert!(find_fibers(&img, 10).is_empty(), "{:?}", find_fibers(&img, 10));
    }

    /// Fibers in a column, a little brighter than a background lit brighter on one side
    /// than the other (the user's 2026-10-27 video): all of them found.
    #[test]
    fn fibers_on_a_slope_of_light() {
        let mut img = RgbImage::from_fn(960, 560, |x, _| image::Rgb([(210.0 - 100.0 * x as f64 / 960.0) as u8; 3]));
        let ys = [70i64, 210, 350, 490];
        for &y in &ys {
            draw::fill_circle_px(&mut img, 480, y, 66, [178, 178, 178]);
            for s in [-1, 1] {
                draw::fill_circle_px(&mut img, 480, y + s * 33, 18, [120, 120, 120]);
            }
        }
        let img = crate::imgproc::gaussian_blur_rgb(&img, 1.5);
        let mut found = find_fibers(&img, 10);
        found.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        assert_eq!(found.len(), 4, "{found:?}");
        for (f, &y) in found.iter().zip(&ys) {
            assert!((f.0 - 480.0).abs() < 3.0 && (f.1 - y as f64).abs() < 3.0 && (f.2 - 66.0).abs() < 4.0, "{found:?}");
        }
    }

    /// A groove full of fibers, each small in the picture (the user's 2026-10-27 captures):
    /// every one found, and no circle round the column.
    #[test]
    fn a_column_of_small_fibers() {
        let mut img = RgbImage::from_fn(1920, 1080, |x, _| image::Rgb([(120.0 - 100.0 * (x as f64 / 1920.0)) as u8; 3]));
        let ys: Vec<i64> = (0..14).map(|k| 104 + 71 * k).collect();
        for &y in &ys {
            draw::fill_circle_px(&mut img, 950, y, 34, [190, 190, 190]);
            for s in [-1, 1] {
                draw::fill_circle_px(&mut img, 950, y + s * 15, 7, [60, 60, 60]);
            }
        }
        let img = crate::imgproc::gaussian_blur_rgb(&img, 1.5);
        let found = find_fibers(&img, MAX_FIBERS);
        assert_eq!(found.len(), 14, "{found:?}");
        assert!(found.iter().all(|f| (f.2 - 34.0).abs() < 3.0 && (f.0 - 950.0).abs() < 3.0), "{found:?}");
        // The user's captures, if here.
        let dir = std::path::Path::new("C:/Users/ethan/Desktop/Fiber Tests");
        for name in ["cap_20261027_175731.tif", "cap_20261027_175752.tif"] {
            if let Some(img) = crate::capture::load_image(&dir.join(name)) {
                assert_eq!(find_fibers(&img, MAX_FIBERS).len(), 14, "{name}");
            }
        }
    }

    #[test]
    fn finds_the_drawn_fiber() {
        let img = drawn_panda(30.0);
        let found = find_fibers(&img, 10);
        assert!(!found.is_empty());
        let (x, y, r) = found[0];
        assert!((x - 200.0).abs() < 2.0 && (y - 200.0).abs() < 2.0 && (r - 150.0).abs() < 3.0, "{found:?}");
    }

    /// A drawn apple core: wide, pinched top and bottom where the rods press (slow axis
    /// through the pinch), its rods too faint to see.
    fn drawn_apple_core(slow: f64) -> RgbImage {
        let (c, r) = (300.0, 250.0);
        let t = slow.to_radians();
        let (sx, sy) = (t.cos(), -t.sin()); // along the slow axis, y down
        let mut img = RgbImage::from_pixel(600, 600, image::Rgb([20, 20, 20]));
        draw::fill_circle_px(&mut img, 300, 300, 250, [150, 150, 150]);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let (px, py) = (x as f64 - c, y as f64 - c);
            let (s, f) = (px * sx + py * sy, -px * sy + py * sx);
            let (w, h) = (0.17 * r, 0.09 * r + 0.05 * r * (f / (0.17 * r)).powi(2));
            if f.abs() < w && s.abs() < h {
                p.0 = [220, 220, 220];
            }
        }
        crate::imgproc::gaussian_blur_rgb(&img, 1.5)
    }

    #[test]
    fn apple_core_gives_the_axes() {
        for truth in [0.0, 30.0, 93.0, 150.0] {
            let img = drawn_apple_core(truth);
            let core = apple_core(&img, 300.0, 300.0, 250.0, None).unwrap();
            assert!(core.clear() && !core.by_rods && angle_error(core.slow_deg, truth) < 1.0, "{truth}: {core:?}");
            // Rod outlines beside it pick the slow axis, whichever way the pinches look;
            // rods far from both of its axes are ignored.
            for (rods, slow) in [(truth + 4.0, truth), (truth + 87.0, truth + 90.0), (truth + 45.0, truth)] {
                let c = apple_core(&img, 300.0, 300.0, 250.0, Some(rods)).unwrap();
                assert!(angle_error(c.slow_deg, slow) < 1.0 && c.by_rods == (rods != truth + 45.0), "{truth} rods {rods}: {c:?}");
            }
            // No rods to see: Automatic falls back on the apple core.
            let f = measure_axis(&img, 300.0, 300.0, 250.0, "auto", false, &Detection::default());
            assert!(f.method == "core" && f.clear() && angle_error(f.slow_deg, truth) < 1.0, "{truth}: {f:?}");
        }
    }

    /// The user's grey, front-lit captures of one Panda fiber (rods about vertical, often
    /// washed out; a sharp apple core): every one found and measured within 4° of the rods'
    /// direction by eye, except two that can't be (rods unclear; only the core lit).
    #[test]
    fn real_grey_captures() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release/captures");
        for name in ["180148", "180150", "180430", "180445", "180452", "180703", "180708", "180712", "180746", "180757",
                     "181139", "181330", "181345", "181355", "181407", "181428", "181720"] {
            let Some(img) = crate::capture::load_image(&dir.join(format!("cap_20261020_{name}.tif"))) else { return }; // not in the repository
            let found = analyse(&img, None, "auto", false, &Detection::default(), false);
            assert_eq!(found.len(), 1, "{name}");
            assert!(found[0].clear() && angle_error(found[0].slow_deg, 93.0) < 4.0, "{name}: {:?}", found[0]);
        }
    }

    /// A grey fiber on a grey background, as when lit from the front: only a faint
    /// edge shows, with or without a lit core that stands out (the 2026-10-20 captures).
    #[test]
    fn finds_a_fiber_by_its_faint_edge() {
        for lit_core in [false, true] {
            let mut img = RgbImage::from_pixel(1280, 720, image::Rgb([150, 150, 150]));
            draw::ring_px(&mut img, 660, 370, 330, 4, [134, 134, 134]);
            for s in [-1, 1] {
                draw::fill_circle_px(&mut img, 660, 370 + s * 165, 95, [140, 140, 140]);
            }
            if lit_core {
                draw::fill_circle_px(&mut img, 660, 370, 45, [255, 255, 255]);
            }
            // Uneven light (brighter on the left, so a brightness threshold splits the image, not the fiber) and a little sensor noise.
            let mut seed = 12345u32;
            for (x, _, p) in img.enumerate_pixels_mut() {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let v = p.0[0] as i32 + 45 - (x as i32 * 90 / 1280) + (seed >> 24) as i32 % 9 - 4;
                let v = v.clamp(0, 255) as u8;
                p.0 = [v, v, v];
            }
            let img = crate::imgproc::gaussian_blur_rgb(&img, 1.5);
            let found = find_fibers(&img, 10);
            assert!(!found.is_empty(), "lit core {lit_core}: nothing found");
            let (x, y, r) = found[0];
            // The cladding (not the core, not nothing), roughly: a thin drawn ring isn't refined
            // like a real edge, so how closely is checked on the real captures by eye.
            assert!((x - 660.0).hypot(y - 370.0) < 33.0 && (r - 330.0).abs() < 33.0, "lit core {lit_core}: {found:?}");
        }
    }

    /// Big rods at 30 deg, and a pair of smaller, crisper rings at 120 deg:
    /// with Smallest rod above the small rings' size, only the big rods count.
    #[test]
    fn smallest_rod_leaves_out_small_rings() {
        let mut img = RgbImage::from_pixel(400, 400, image::Rgb([20, 20, 20]));
        draw::fill_circle_px(&mut img, 200, 200, 150, [200, 200, 200]);
        for (deg, gap, r, gray) in [(30.0f64, 85.0, 50, 175u8), (120.0, 60.0, 28, 0)] {
            let a = deg.to_radians();
            for s in [-1.0, 1.0] {
                let (x, y) = (200.0 + s * gap * a.cos(), 200.0 - s * gap * a.sin());
                draw::ring_px(&mut img, rne(x) as i64, rne(y) as i64, r, 3, [gray, gray, gray]);
            }
        }
        let img = crate::imgproc::gaussian_blur_rgb(&img, 1.2);
        let picky = Detection { min_rod: 30.0, ..Default::default() };
        let f = measure_axis(&img, 200.0, 200.0, 150.0, "rods", false, &picky);
        assert!(angle_error(f.slow_deg, 30.0) < 1.0, "{}", f.slow_deg);
        assert!(f.rod_radii.iter().all(|&r| r >= 0.3 * 150.0 - 0.5), "{:?}", f.rod_radii);
    }

    #[test]
    fn same_fibers_within_a_small_move() {
        let a = [(100.0, 100.0, 50.0), (220.0, 100.0, 50.0)];
        assert!(same_fibers(&a, &[(222.0, 101.0, 51.0), (103.0, 99.0, 49.0)]), "order doesn't matter");
        assert!(!same_fibers(&a, &[(100.0, 100.0, 50.0)]), "a fiber lost");
        assert!(!same_fibers(&a, &[(100.0, 100.0, 50.0), (240.0, 100.0, 50.0)]), "one moved a lot");
        assert!(!same_fibers(&a, &[(100.0, 100.0, 50.0), (220.0, 100.0, 60.0)]), "one a different size");
        assert!(!same_fibers(&[], &[]), "nothing found isn't a lock");
    }

    #[test]
    fn brightest_centre_counted_from_the_right() {
        let mut img = RgbImage::from_pixel(900, 300, image::Rgb([10, 10, 10]));
        for (x, core) in [(150, 120u8), (450, 250), (750, 160)] {
            draw::fill_circle_px(&mut img, x, 150, 130, [90, 90, 90]);
            draw::fill_circle_px(&mut img, x, 150, 6, [core, core, core]);
        }
        img.put_pixel(150, 150, image::Rgb([255, 255, 255])); // one hot pixel doesn't count
        let mut axes: Vec<FiberAxis> = [150.0, 450.0, 750.0]
            .iter()
            .map(|&x| {
                let mut f = FiberAxis::new(x, 150.0, 130.0, 0.0, 1.0, "?", 0);
                f.centre_brightness = Some(centre_brightness(&img, x, 150.0, 130.0));
                f
            })
            .collect();
        let b: Vec<f64> = axes.iter().map(|f| f.centre_brightness.unwrap()).collect();
        assert!((b[1] - 250.0).abs() < 1.0 && b[0] < 200.0, "{b:?}");
        assert_eq!(brightest_centre(&axes), Some((1, 2)));
        axes[2].centre_brightness = Some(255.0);
        assert_eq!(brightest_centre(&axes), Some((2, 1)));
        assert_eq!(brightest_centre(&axes[..1]), None);
    }
}
