//! Image quality round one fiber: contrast against the background, focus
//! (how wide the cladding edge is), noise and exposure (clipped or dark
//! pixels), and a Good / Fair / Poor rating from the worst of them.
//!
//! All levels are luma (0-255). Fiber and background are medians, so dust or a
//! bright core spot doesn't move them. Pixels inside neighbouring fibers (side
//! by side in a V-groove) don't count as background.

use image::RgbImage;

use crate::imgproc as ip;

const FIBER_IN: f64 = 0.9; // fiber level: inside this fraction of the radius
const RING: (f64, f64) = (1.15, 1.5); // background level: this ring (fractions of the radius)
const EDGE: (f64, f64) = (0.85, 1.15); // the cladding edge is measured across this band
const SECTORS: usize = 24; // edge profiles averaged per sector, then the median sector taken
const RAYS_PER_SECTOR: usize = 8;
const CLIP_HIGH: f32 = 250.0;
const BLACK: f64 = 5.0; // a background this dark is clipped at black: its noise can't be seen
const PEAK: f64 = 95.0; // the fiber's bright parts: this percentile
const MIN_NOISE: f64 = 0.5; // 8-bit steps: below this the noise can't be told apart

/// (good up to, fair up to) for each indicator; past the second it's Poor.
pub const CONTRAST: (f64, f64) = (0.20, 0.08); // |Michelson contrast|, at least
pub const SNR: (f64, f64) = (20.0, 8.0); // at least
pub const EDGE_WIDTH: (f64, f64) = (0.05, 0.10); // edge width / radius, at most
pub const CLIPPED: (f64, f64) = (0.01, 0.10); // share of fiber pixels at full white, at most
pub const BRIGHT: (f64, f64) = (60.0, 30.0); // the fiber's bright parts (PEAK percentile luma), at least

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Grade {
    Good,
    Fair,
    Poor,
}

impl Grade {
    pub fn label(self) -> &'static str {
        match self {
            Grade::Good => "Good",
            Grade::Fair => "Fair",
            Grade::Poor => "Poor",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FiberQuality {
    /// Median luma inside the fiber.
    pub fiber: f64,
    /// Median luma of the ring round it.
    pub background: f64,
    /// Michelson contrast (fiber - background) / (fiber + background), -1 to 1; + = fiber brighter.
    pub contrast: f64,
    /// Pixels the cladding edge takes to go from fiber to background (step / steepest slope).
    /// None when there is no edge to measure.
    pub edge_px: Option<f64>,
    /// Pixel noise of the background (robust standard deviation, luma). None
    /// when the background is clipped at black, so the noise doesn't show.
    pub noise: Option<f64>,
    /// Share of fiber pixels at full white (>= 250). Dark ones aren't counted:
    /// stress rods are often black.
    pub clipped: f64,
    /// Luma of the fiber's bright parts (95th percentile): low = underexposed.
    pub peak: f64,
    /// The fiber radius it was measured at (px), for the edge width relative to it.
    pub radius: f64,
}

impl FiberQuality {
    /// Contrast step over the noise (None when the noise can't be measured).
    pub fn snr(&self) -> Option<f64> {
        self.noise.map(|n| (self.fiber - self.background).abs() / n.max(MIN_NOISE))
    }

    /// The worst indicator's grade, and what pulled it down.
    pub fn grade(&self) -> (Grade, Vec<String>) {
        let at_least = |v: f64, (good, fair): (f64, f64)| if v >= good { Grade::Good } else if v >= fair { Grade::Fair } else { Grade::Poor };
        let at_most = |v: f64, (good, fair): (f64, f64)| if v <= good { Grade::Good } else if v <= fair { Grade::Fair } else { Grade::Poor };
        let mut checks = vec![(at_least(self.contrast.abs(), CONTRAST), "low contrast".to_string())];
        if let Some(snr) = self.snr() {
            checks.push((at_least(snr, SNR), "noisy".to_string()));
        }
        if let Some(e) = self.edge_px {
            checks.push((at_most(e / self.radius.max(1.0), EDGE_WIDTH), "soft edge (out of focus?)".to_string()));
        }
        checks.push((at_most(self.clipped, CLIPPED), format!("overexposed ({:.0}% clipped)", 100.0 * self.clipped)));
        checks.push((at_least(self.peak, BRIGHT), "dark (underexposed?)".to_string()));
        let worst = checks.iter().map(|c| c.0).max().unwrap_or(Grade::Good);
        let why = checks.into_iter().filter(|c| worst > Grade::Good && c.0 == worst).map(|c| c.1).collect();
        (worst, why)
    }

    /// One line per indicator, for hover text.
    pub fn details(&self) -> String {
        let (g, why) = self.grade();
        let mut s = g.label().to_string();
        if !why.is_empty() {
            s += &format!(": {}", why.join(", "));
        }
        s += &format!("\nContrast {:+.0}% (fiber {:.0}, background {:.0})", 100.0 * self.contrast, self.fiber, self.background);
        s += &match self.edge_px {
            Some(e) => format!("\nEdge width {e:.1} px ({:.1}% of the radius)", 100.0 * e / self.radius.max(1.0)),
            None => "\nEdge width: no edge found".into(),
        };
        s += &match (self.noise, self.snr()) {
            (Some(n), Some(snr)) => format!("\nNoise {n:.1}, SNR {snr:.0}"),
            _ => "\nNoise: can't be seen on a black background".into(),
        };
        s += &format!("\nClipped {:.1}%, bright parts {:.0}", 100.0 * self.clipped, self.peak);
        s
    }
}

fn luma(p: [u8; 3]) -> f32 {
    0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
}

/// The quality round the fiber at (cx, cy, radius); `others` are the other fibers'
/// circles, kept out of the background. None if too little of it is on the image.
pub fn fiber_quality(image: &RgbImage, cx: f64, cy: f64, radius: f64, others: &[(f64, f64, f64)]) -> Option<FiberQuality> {
    if radius < 4.0 {
        return None;
    }
    let (w, h) = (image.width() as i64, image.height() as i64);
    let reach = RING.1 * radius;
    let (x0, x1) = (((cx - reach).floor() as i64).max(0), ((cx + reach).ceil() as i64 + 1).min(w));
    let (y0, y1) = (((cy - reach).floor() as i64).max(0), ((cy + reach).ceil() as i64 + 1).min(h));
    if x1 - x0 < 3 || y1 - y0 < 3 {
        return None;
    }
    let (cw, ch) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let lum = ip::ImgF::from_fn(cw, ch, |x, y| luma(image.get_pixel((x0 + x as i64) as u32, (y0 + y as i64) as u32).0));
    let in_other = |x: f64, y: f64, grow: f64| others.iter().any(|&(ox, oy, or)| (x - ox).hypot(y - oy) < grow * or);

    let mut fiber = Vec::new();
    let mut high = 0usize;
    let mut is_bg = vec![false; cw * ch];
    let mut bg = Vec::new();
    for y in 0..ch {
        for x in 0..cw {
            let (px, py) = ((x0 + x as i64) as f64 + 0.5, (y0 + y as i64) as f64 + 0.5);
            let d = (px - cx).hypot(py - cy) / radius;
            let v = lum.at(x, y);
            if d <= FIBER_IN {
                fiber.push(v);
                high += (v >= CLIP_HIGH) as usize;
            } else if (RING.0..=RING.1).contains(&d) && !in_other(px, py, 1.05) {
                bg.push(v);
                is_bg[y * cw + x] = true;
            }
        }
    }
    if fiber.len() < 20 || bg.len() < 20 {
        return None;
    }
    let (f, b) = (ip::median(&fiber) as f64, ip::median(&bg) as f64);
    let contrast = if f + b > 0.0 { (f - b) / (f + b) } else { 0.0 };

    // Noise from neighbouring background pixels' differences, so a smooth slope
    // across the background (groove walls, uneven light) doesn't count as noise.
    let mut diffs = Vec::new();
    for y in 0..ch {
        for x in 0..cw - 1 {
            if is_bg[y * cw + x] && is_bg[y * cw + x + 1] {
                diffs.push(lum.at(x + 1, y) - lum.at(x, y));
            }
        }
    }
    let noise = (diffs.len() >= 10 && b > BLACK).then(|| {
        let m = ip::median(&diffs);
        let dev: Vec<f32> = diffs.iter().map(|d| (d - m).abs()).collect();
        1.4826 * ip::median(&dev) as f64 / std::f64::consts::SQRT_2
    });

    let edge_px = edge_width(&lum, cx - x0 as f64, cy - y0 as f64, radius, |x, y| in_other(x + x0 as f64, y + y0 as f64, 1.02));
    Some(FiberQuality {
        fiber: f,
        background: b,
        contrast,
        edge_px,
        noise,
        clipped: high as f64 / fiber.len() as f64,
        peak: ip::percentile(&fiber, PEAK) as f64,
        radius,
    })
}

/// Width (px) of the cladding edge: per sector, the profile across the edge
/// averaged over a few rays, its step divided by its steepest slope (a blurred
/// step of width w rises by step/w per pixel at its middle). The median sector.
/// Rays off the image or into another fiber are left out.
fn edge_width(lum: &ip::ImgF, cx: f64, cy: f64, radius: f64, blocked: impl Fn(f64, f64) -> bool) -> Option<f64> {
    edge_width_across(lum, cx, cy, radius, EDGE.0 * radius, EDGE.1 * radius, blocked)
}

/// As edge_width, the profiles from `from` to `to` px from the center.
fn edge_width_across(lum: &ip::ImgF, cx: f64, cy: f64, radius: f64, from: f64, to: f64, blocked: impl Fn(f64, f64) -> bool) -> Option<f64> {
    let step = (0.25f64).max((to - from) / 400.0);
    let n = ((to - from) / step) as usize + 1;
    let end = (n / 10).max(2); // samples averaged at each end for the step
    let mut widths = Vec::new();
    for s in 0..SECTORS {
        let mut profile = vec![0f64; n];
        let mut rays = 0;
        'ray: for k in 0..RAYS_PER_SECTOR {
            let a = (s as f64 + (k as f64 + 0.5) / RAYS_PER_SECTOR as f64) * std::f64::consts::TAU / SECTORS as f64;
            let (dx, dy) = (a.cos(), a.sin());
            let mut ray = Vec::with_capacity(n);
            for i in 0..n {
                let t = from + i as f64 * step;
                let (x, y) = (cx + t * dx, cy + t * dy);
                if x < 0.0 || y < 0.0 || x > (lum.w - 1) as f64 || y > (lum.h - 1) as f64 || (t >= radius && blocked(x, y)) {
                    continue 'ray;
                }
                ray.push(ip::sample(lum, x as f32, y as f32) as f64);
            }
            for (p, v) in profile.iter_mut().zip(ray) {
                *p += v;
            }
            rays += 1;
        }
        if rays < RAYS_PER_SECTOR / 2 {
            continue;
        }
        let profile: Vec<f64> = profile.iter().map(|v| v / rays as f64).collect();
        let inner = profile[..end].iter().sum::<f64>() / end as f64;
        let outer = profile[n - end..].iter().sum::<f64>() / end as f64;
        let rise = outer - inner;
        if rise.abs() < 2.0 {
            continue; // no edge in this sector
        }
        // Steepest slope in the direction of the step, over one pixel (3-sample smoothing first).
        let smooth: Vec<f64> = (0..n).map(|i| {
            let (a, b) = (i.saturating_sub(1), (i + 1).min(n - 1));
            (profile[a] + profile[i] + profile[b]) / 3.0
        }).collect();
        let gap = ((1.0 / step).round() as usize).max(1);
        let slope = (0..n.saturating_sub(gap)).map(|i| (smooth[i + gap] - smooth[i]) * rise.signum() / (gap as f64 * step)).fold(0.0, f64::max);
        if slope > 0.0 {
            widths.push(rise.abs() / slope);
        }
    }
    (widths.len() >= SECTORS / 4).then(|| ip::median_f64(&widths))
}

/// Focus on the stress rods: how many pixels their edges take to go from rod to cladding
/// (as edge_width, across each rod's outline), the rods averaged. Smaller is sharper. None if
/// no rod edge could be measured. `rods`: (center x, center y, radius).
pub fn rod_edge_width(image: &RgbImage, rods: &[(f64, f64, f64)]) -> Option<f64> {
    let widths: Vec<f64> = rods.iter().filter(|r| r.2 >= 3.0).filter_map(|&(cx, cy, r)| {
        let half = (0.3 * r).max(6.0);
        let reach = r + half + 2.0;
        let (w, h) = (image.width() as i64, image.height() as i64);
        let (x0, y0) = (((cx - reach).floor() as i64).max(0), ((cy - reach).floor() as i64).max(0));
        let (x1, y1) = (((cx + reach).ceil() as i64 + 1).min(w), ((cy + reach).ceil() as i64 + 1).min(h));
        if x1 - x0 < 4 || y1 - y0 < 4 {
            return None;
        }
        let lum = ip::ImgF::from_fn((x1 - x0) as usize, (y1 - y0) as usize, |x, y| luma(image.get_pixel((x0 + x as i64) as u32, (y0 + y as i64) as u32).0));
        edge_width_across(&lum, cx - x0 as f64, cy - y0 as f64, r, (r - half).max(1.0), r + half, |_, _| false)
    }).collect();
    (!widths.is_empty()).then(|| widths.iter().sum::<f64>() / widths.len() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rod_edges_sharp_and_blurred() {
        let mut img = RgbImage::from_pixel(400, 400, image::Rgb([30; 3]));
        draw::fill_circle_px(&mut img, 200, 200, 150, [200; 3]);
        for x in [125, 275] {
            draw::fill_circle_px(&mut img, x, 200, 35, [60; 3]);
        }
        let rods = [(125.0, 200.0, 35.0), (275.0, 200.0, 35.0)];
        let sharp = rod_edge_width(&img, &rods).unwrap();
        let soft = rod_edge_width(&ip::gaussian_blur_rgb(&img, 3.0), &rods).unwrap();
        assert!(sharp < 2.0, "{sharp}");
        assert!((soft - 3.0 * std::f64::consts::TAU.sqrt()).abs() < 2.0, "sigma 3 -> about 7.5 px, got {soft}");
        assert_eq!(rod_edge_width(&img, &[]), None);
    }
    use crate::draw;

    fn disc(fiber: u8, background: u8) -> RgbImage {
        let mut img = RgbImage::from_pixel(300, 300, image::Rgb([background; 3]));
        draw::fill_circle_px(&mut img, 150, 150, 80, [fiber; 3]);
        img
    }

    fn noisy(img: &mut RgbImage, amount: f64) {
        let mut seed = 12345u64;
        for p in img.pixels_mut() {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let n = ((seed >> 33) as f64 / (1u64 << 31) as f64 - 0.5) * 2.0 * amount * 3f64.sqrt(); // uniform, std = amount
            for c in p.0.iter_mut() {
                *c = (*c as f64 + n).round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    #[test]
    fn contrast_both_ways() {
        let q = fiber_quality(&disc(200, 50), 150.0, 150.0, 80.0, &[]).unwrap();
        assert!((q.fiber - 200.0).abs() < 1.0 && (q.background - 50.0).abs() < 1.0, "{q:?}");
        assert!((q.contrast - 0.6).abs() < 0.01, "{}", q.contrast);
        assert_eq!(q.grade().0, Grade::Good, "{}", q.details());
        let dark = fiber_quality(&disc(60, 180), 150.0, 150.0, 80.0, &[]).unwrap();
        assert!(dark.contrast < -0.4, "{}", dark.contrast);
        let faint = fiber_quality(&disc(105, 100), 150.0, 150.0, 80.0, &[]).unwrap();
        assert_eq!(faint.grade().0, Grade::Poor, "{}", faint.details());
        assert!(faint.grade().1.iter().any(|r| r.contains("contrast")));
    }

    #[test]
    fn blur_widens_the_edge() {
        let sharp = fiber_quality(&disc(200, 50), 150.0, 150.0, 80.0, &[]).unwrap().edge_px.unwrap();
        let blurred_img = ip::gaussian_blur_rgb(&disc(200, 50), 4.0);
        let blurred = fiber_quality(&blurred_img, 150.0, 150.0, 80.0, &[]).unwrap();
        let e = blurred.edge_px.unwrap();
        assert!(sharp < 2.0, "{sharp}");
        assert!((e - 4.0 * (std::f64::consts::TAU).sqrt()).abs() < 2.0, "sigma 4 -> about 10 px, got {e}");
        assert!(blurred.grade().1.iter().any(|r| r.contains("focus")), "{}", blurred.details());
    }

    #[test]
    fn noise_and_clipping() {
        let mut img = disc(150, 50);
        noisy(&mut img, 8.0);
        let q = fiber_quality(&img, 150.0, 150.0, 80.0, &[]).unwrap();
        assert!((q.noise.unwrap() - 8.0).abs() < 1.5, "{:?}", q.noise);
        assert_eq!(q.grade().0, Grade::Fair, "SNR about 12: {}", q.details());
        let bright = fiber_quality(&disc(255, 50), 150.0, 150.0, 80.0, &[]).unwrap();
        assert!(bright.clipped > 0.99 && bright.grade().0 == Grade::Poor, "{}", bright.details());
        assert!(bright.grade().1[0].starts_with("overexposed"));
        let dim = fiber_quality(&disc(25, 8), 150.0, 150.0, 80.0, &[]).unwrap();
        assert!(dim.grade().0 == Grade::Poor && dim.grade().1.iter().any(|r| r.contains("dark")), "{}", dim.details());
    }

    /// Black stress rods aren't underexposure, and noise on a black background can't be measured.
    #[test]
    fn black_rods_and_background() {
        let mut img = disc(200, 0);
        for x in [100, 200] {
            draw::fill_circle_px(&mut img, x, 150, 20, [0; 3]);
        }
        let q = fiber_quality(&img, 150.0, 150.0, 80.0, &[]).unwrap();
        assert!(q.noise.is_none() && q.snr().is_none(), "{q:?}");
        assert_eq!(q.grade().0, Grade::Good, "{}", q.details());
    }

    /// Fibers touching in a V-groove: the neighbour's pixels aren't background.
    #[test]
    fn neighbours_are_not_background() {
        let mut img = RgbImage::from_pixel(500, 200, image::Rgb([40; 3]));
        for x in [90, 250, 410] {
            draw::fill_circle_px(&mut img, x, 100, 80, [200; 3]);
        }
        let others = [(90.0, 100.0, 80.0), (410.0, 100.0, 80.0)];
        let q = fiber_quality(&img, 250.0, 100.0, 80.0, &others).unwrap();
        assert!((q.background - 40.0).abs() < 1.0, "{q:?}");
        assert!(q.edge_px.unwrap() < 2.0, "{q:?}");
        assert_eq!(q.grade().0, Grade::Good, "{}", q.details());
    }
}
