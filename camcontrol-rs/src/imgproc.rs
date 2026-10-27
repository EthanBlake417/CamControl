//! Image operations the processing code needs, written to match OpenCV's
//! (the Python app's) behaviour: the same kernels, border rules and rounding,
//! so results agree with the Python version to a fraction of a pixel.
//!
//! Images: `RgbImage` / `GrayImage` (8-bit, from the image crate) for pixels
//! people see, and `ImgF` (one f32 channel) for everything in between.

use image::{GrayImage, Rgb, RgbImage};

/// One channel of f32 values, row by row.
#[derive(Clone, Debug, PartialEq)]
pub struct ImgF {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl ImgF {
    pub fn new(w: usize, h: usize) -> Self {
        Self { w, h, data: vec![0.0; w * h] }
    }

    pub fn filled(w: usize, h: usize, v: f32) -> Self {
        Self { w, h, data: vec![v; w * h] }
    }

    pub fn from_fn(w: usize, h: usize, f: impl Fn(usize, usize) -> f32) -> Self {
        let mut data = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                data.push(f(x, y));
            }
        }
        Self { w, h, data }
    }

    /// As from_fn, on all cores.
    pub fn par_from_fn(w: usize, h: usize, f: impl Fn(usize, usize) -> f32 + Sync) -> Self {
        let mut data = vec![0f32; w * h];
        par_rows(&mut data, w, |y, row| {
            for (x, v) in row.iter_mut().enumerate() {
                *v = f(x, y);
            }
        });
        Self { w, h, data }
    }

    pub fn from_gray(g: &GrayImage) -> Self {
        Self { w: g.width() as usize, h: g.height() as usize, data: g.as_raw().iter().map(|&v| v as f32).collect() }
    }

    /// Rounded and clipped to 0-255, as OpenCV does when it writes 8-bit pixels.
    pub fn to_gray(&self) -> GrayImage {
        GrayImage::from_raw(self.w as u32, self.h as u32, self.data.iter().map(|&v| sat_u8(v)).collect()).unwrap()
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.w + x]
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, v: f32) {
        self.data[y * self.w + x] = v;
    }

    pub fn map(&self, f: impl Fn(f32) -> f32) -> Self {
        Self { w: self.w, h: self.h, data: self.data.iter().map(|&v| f(v)).collect() }
    }

    pub fn zip(&self, other: &ImgF, f: impl Fn(f32, f32) -> f32) -> Self {
        assert_eq!((self.w, self.h), (other.w, other.h));
        Self { w: self.w, h: self.h, data: self.data.iter().zip(&other.data).map(|(&a, &b)| f(a, b)).collect() }
    }

    pub fn max_value(&self) -> f32 {
        self.data.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
    }

    /// (x, y) of the largest value (the first, in row order).
    pub fn argmax(&self) -> (usize, usize) {
        let mut best = 0;
        for (i, &v) in self.data.iter().enumerate() {
            if v > self.data[best] {
                best = i;
            }
        }
        (best % self.w, best / self.w)
    }

    pub fn crop(&self, x0: usize, y0: usize, w: usize, h: usize) -> Self {
        Self::from_fn(w, h, |x, y| self.at(x0 + x, y0 + y))
    }
}

/// Run f(y, row) for every row of a w-wide buffer, on all cores (in bands of rows).
pub fn par_rows<T: Send>(data: &mut [T], w: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    par_rows_min(data, w, 20_000, f);
}

/// As par_rows, splitting even small outputs (at least min_len values) when each value is a lot of work.
pub fn par_rows_min<T: Send>(data: &mut [T], w: usize, min_len: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    let h = if w == 0 { 0 } else { data.len() / w };
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16);
    if h * w < min_len || threads == 1 || h < 2 {
        for (y, row) in data.chunks_mut(w.max(1)).enumerate() {
            f(y, row);
        }
        return;
    }
    let band = h.div_ceil(threads);
    std::thread::scope(|s| {
        for (b, chunk) in data.chunks_mut(band * w).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (i, row) in chunk.chunks_mut(w).enumerate() {
                    f(b * band + i, row);
                }
            });
        }
    });
}

/// Round half away from zero and clip to 0-255 (OpenCV's saturate_cast<uchar>).
#[inline]
pub fn sat_u8(v: f32) -> u8 {
    if v.is_nan() {
        return 0;
    }
    v.round().clamp(0.0, 255.0) as u8
}

/// OpenCV's BORDER_REFLECT_101: ...dcb|abcd|cba...
#[inline]
pub fn reflect101(mut i: isize, n: usize) -> usize {
    let n = n as isize;
    if n == 1 {
        return 0;
    }
    loop {
        if i < 0 {
            i = -i;
        } else if i >= n {
            i = 2 * n - 2 - i;
        } else {
            return i as usize;
        }
    }
}

#[inline]
fn replicate(i: isize, n: usize) -> usize {
    i.clamp(0, n as isize - 1) as usize
}

// --- color ------------------------------------------------------------------------

/// Gray from RGB with OpenCV's weights (BGR2GRAY), rounded.
pub fn to_gray(img: &RgbImage) -> GrayImage {
    let mut out = GrayImage::new(img.width(), img.height());
    for (o, p) in out.pixels_mut().zip(img.pixels()) {
        let [r, g, b] = p.0;
        // OpenCV's fixed-point weights (R 4899, G 9617, B 1868 of 2^14).
        o.0[0] = ((r as u32 * 4899 + g as u32 * 9617 + b as u32 * 1868 + (1 << 13)) >> 14) as u8;
    }
    out
}

pub fn gray_to_rgb(g: &GrayImage) -> RgbImage {
    RgbImage::from_fn(g.width(), g.height(), |x, y| {
        let v = g.get_pixel(x, y).0[0];
        Rgb([v, v, v])
    })
}

/// The three channels as f32 images (R, G, B).
pub fn split(img: &RgbImage) -> [ImgF; 3] {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut ch = [ImgF::new(w, h), ImgF::new(w, h), ImgF::new(w, h)];
    for (i, p) in img.pixels().enumerate() {
        for c in 0..3 {
            ch[c].data[i] = p.0[c] as f32;
        }
    }
    ch
}

pub fn merge(ch: &[ImgF; 3]) -> RgbImage {
    let (w, h) = (ch[0].w, ch[0].h);
    let mut out = RgbImage::new(w as u32, h as u32);
    for (i, p) in out.pixels_mut().enumerate() {
        p.0 = [sat_u8(ch[0].data[i]), sat_u8(ch[1].data[i]), sat_u8(ch[2].data[i])];
    }
    out
}

/// CIE L*a*b* as OpenCV's 8-bit BGR2LAB gives it (L 0-255, a and b offset by
/// 128), as f32 channels [L, a, b].
pub fn to_lab(img: &RgbImage) -> [ImgF; 3] {
    // sRGB to linear, by table (as OpenCV does for 8-bit images).
    let mut lin = [0f32; 256];
    for (i, v) in lin.iter_mut().enumerate() {
        let x = i as f32 / 255.0;
        *v = if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) };
    }
    let f = |t: f32| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    let (w, h) = (img.width() as usize, img.height() as usize);
    let raw = img.as_raw();
    let mut lab = vec![[0f32; 3]; w * h];
    par_rows(&mut lab, w, |yy, row| {
        for (xx, o) in row.iter_mut().enumerate() {
            let i = (yy * w + xx) * 3;
            let (r, g, b) = (lin[raw[i] as usize], lin[raw[i + 1] as usize], lin[raw[i + 2] as usize]);
            let x = (0.412453 * r + 0.357580 * g + 0.180423 * b) / 0.950456;
            let y = 0.212671 * r + 0.715160 * g + 0.072169 * b;
            let z = (0.019334 * r + 0.119193 * g + 0.950227 * b) / 1.088754;
            let (fx, fy, fz) = (f(x), f(y), f(z));
            let l = if y > 0.008856 { 116.0 * fy - 16.0 } else { 903.3 * y };
            *o = [sat_u8(l * 255.0 / 100.0) as f32, sat_u8(500.0 * (fx - fy) + 128.0) as f32, sat_u8(200.0 * (fy - fz) + 128.0) as f32];
        }
    });
    let mut out = [ImgF::new(w, h), ImgF::new(w, h), ImgF::new(w, h)];
    for (i, v) in lab.iter().enumerate() {
        for c in 0..3 {
            out[c].data[i] = v[c];
        }
    }
    out
}

// --- filters ----------------------------------------------------------------------

/// OpenCV's Gaussian kernel for sigma (ksize chosen as GaussianBlur does for
/// ksize (0, 0): 8-bit images use 3 sigma, float images 4).
pub fn gaussian_kernel(sigma: f64, eight_bit: bool) -> Vec<f32> {
    let k = ((sigma * if eight_bit { 3.0 } else { 4.0 } * 2.0 + 1.0).round() as usize) | 1;
    gaussian_kernel_n(k, sigma)
}

/// OpenCV's getGaussianKernel(n, sigma): sigma <= 0 means "from n", and the
/// small sizes use its fixed tables.
pub fn gaussian_kernel_n(n: usize, sigma: f64) -> Vec<f32> {
    if sigma <= 0.0 {
        match n {
            1 => return vec![1.0],
            3 => return vec![0.25, 0.5, 0.25],
            5 => return vec![0.0625, 0.25, 0.375, 0.25, 0.0625],
            7 => return vec![0.03125, 0.109375, 0.21875, 0.28125, 0.21875, 0.109375, 0.03125],
            _ => {}
        }
    }
    let sigma = if sigma > 0.0 { sigma } else { ((n as f64 - 1.0) * 0.5 - 1.0) * 0.3 + 0.8 };
    let half = (n as f64 - 1.0) / 2.0;
    let mut k: Vec<f64> = (0..n).map(|i| (-((i as f64 - half).powi(2)) / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f64 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= sum);
    k.into_iter().map(|v| v as f32).collect()
}

/// Separable filter, BORDER_REFLECT_101 (OpenCV's default).
pub fn sep_filter(src: &ImgF, kx: &[f32], ky: &[f32]) -> ImgF {
    let (w, h) = (src.w, src.h);
    let (rx, ry) = (kx.len() / 2, ky.len() / 2);
    let mut tmp = ImgF::new(w, h);
    let cols: Vec<usize> = (0..w + 2 * rx).map(|i| reflect101(i as isize - rx as isize, w)).collect();
    par_rows(&mut tmp.data, w, |y, out| {
        let s = &src.data[y * w..(y + 1) * w];
        let row: Vec<f32> = cols.iter().map(|&i| s[i]).collect();
        // Kernel taps outside, pixels inside: the inner loop vectorises.
        for (k, &c) in kx.iter().enumerate() {
            for (o, &v) in out.iter_mut().zip(&row[k..k + w]) {
                *o += c * v;
            }
        }
    });
    let mut out = ImgF::new(w, h);
    let rows: Vec<usize> = (0..h + 2 * ry).map(|i| reflect101(i as isize - ry as isize, h)).collect();
    par_rows(&mut out.data, w, |y, o| {
        for (k, &c) in ky.iter().enumerate() {
            let s = &tmp.data[rows[y + k] * w..(rows[y + k] + 1) * w];
            for (a, &v) in o.iter_mut().zip(s) {
                *a += c * v;
            }
        }
    });
    out
}

/// GaussianBlur(src, (0, 0), sigma) on float data.
pub fn gaussian_blur(src: &ImgF, sigma: f64) -> ImgF {
    let k = gaussian_kernel(sigma, false);
    sep_filter(src, &k, &k)
}

/// GaussianBlur(src, (0, 0), sigma) on 8-bit data: 3-sigma kernel, rounded result.
pub fn gaussian_blur_u8(src: &GrayImage, sigma: f64) -> GrayImage {
    let k = gaussian_kernel(sigma, true);
    sep_filter(&ImgF::from_gray(src), &k, &k).to_gray()
}

/// As gaussian_blur_u8, on each channel.
pub fn gaussian_blur_rgb(src: &RgbImage, sigma: f64) -> RgbImage {
    let k = gaussian_kernel(sigma, true);
    let ch = split(src);
    merge(&[sep_filter(&ch[0], &k, &k), sep_filter(&ch[1], &k, &k), sep_filter(&ch[2], &k, &k)])
}

/// Sobel 3x3 (dx, dy each 0 or 1), as cv2.Sobel(src, CV_32F, dx, dy).
pub fn sobel(src: &ImgF, dx: bool) -> ImgF {
    let d = [-1.0, 0.0, 1.0];
    let s = [1.0, 2.0, 1.0];
    if dx { sep_filter(src, &d, &s) } else { sep_filter(src, &s, &d) }
}

/// cv2.Laplacian(src, CV_32F, ksize=3): kernel [2 0 2; 0 -8 0; 2 0 2].
pub fn laplacian3(src: &ImgF) -> ImgF {
    let (w, h) = (src.w, src.h);
    ImgF::from_fn(w, h, |x, y| {
        let p = |dx: isize, dy: isize| {
            src.at(reflect101(x as isize + dx, w), reflect101(y as isize + dy, h))
        };
        2.0 * (p(-1, -1) + p(1, -1) + p(-1, 1) + p(1, 1)) - 8.0 * p(0, 0)
    })
}

/// Median of each k x k square (k odd), BORDER_REPLICATE, as cv2.medianBlur.
pub fn median_blur(src: &GrayImage, k: usize) -> GrayImage {
    let (w, h) = (src.width() as usize, src.height() as usize);
    let r = (k / 2) as isize;
    let raw = src.as_raw();
    let half = (k * k / 2) as i32; // the median is the (half+1)-th smallest
    let mut out = vec![0u8; w * h];
    par_rows(&mut out, w, |y, row| {
        // Huang's sliding histogram along the row, with the median and the
        // count below it kept up to date (no rescan of the histogram).
        let mut hist = [0i32; 256];
        let (mut med, mut below) = (0usize, 0i32); // below: values < med
        let col = |x: isize| (-r..=r).map(move |dy| raw[replicate(y as isize + dy, h) * w + replicate(x, w)] as usize);
        for dx in -r..=r {
            for v in col(dx) {
                hist[v] += 1;
            }
        }
        for x in 0..w {
            if x > 0 {
                for v in col(x as isize - 1 - r) {
                    hist[v] -= 1;
                    if v < med {
                        below -= 1;
                    }
                }
                for v in col(x as isize + r) {
                    hist[v] += 1;
                    if v < med {
                        below += 1;
                    }
                }
            }
            // Move med so that below <= half < below + hist[med].
            while below > half {
                med -= 1;
                below -= hist[med];
            }
            while below + hist[med] <= half {
                below += hist[med];
                med += 1;
            }
            row[x] = med as u8;
        }
    });
    GrayImage::from_raw(w as u32, h as u32, out).unwrap()
}

// --- thresholds and shapes ------------------------------------------------------------

/// Otsu's threshold of an 8-bit image (as cv2.threshold with THRESH_OTSU): pixels
/// above it are foreground.
pub fn otsu_u8(img: &GrayImage) -> u8 {
    let mut hist = [0f64; 256];
    for &v in img.as_raw() {
        hist[v as usize] += 1.0;
    }
    let total: f64 = hist.iter().sum();
    let mu: f64 = hist.iter().enumerate().map(|(i, &n)| i as f64 * n).sum::<f64>() / total;
    let (mut q1, mut mu1, mut best, mut best_t) = (0f64, 0f64, 0f64, 0u8);
    for i in 0..256 {
        let p = hist[i] / total;
        let q2 = q1 + p;
        if q2 > f64::EPSILON && 1.0 - q2 > f64::EPSILON {
            mu1 = (mu1 * q1 + i as f64 * p) / q2;
            let mu2 = (mu - q2 * mu1) / (1.0 - q2);
            let between = q2 * (1.0 - q2) * (mu1 - mu2) * (mu1 - mu2);
            if between > best {
                best = between;
                best_t = i as u8;
            }
        } else if q2 > f64::EPSILON {
            mu1 = (mu1 * q1 + i as f64 * p) / q2;
        }
        q1 = q2;
    }
    best_t
}

/// A boolean mask (true = set) of size w x h.
#[derive(Clone, Debug, PartialEq)]
pub struct Mask {
    pub w: usize,
    pub h: usize,
    pub data: Vec<bool>,
}

impl Mask {
    pub fn new(w: usize, h: usize) -> Self {
        Self { w, h, data: vec![false; w * h] }
    }

    pub fn from_fn(w: usize, h: usize, f: impl Fn(usize, usize) -> bool) -> Self {
        let mut data = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                data.push(f(x, y));
            }
        }
        Self { w, h, data }
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> bool {
        self.data[y * self.w + x]
    }

    pub fn count(&self) -> usize {
        self.data.iter().filter(|&&v| v).count()
    }

    pub fn not(&self) -> Self {
        Self { w: self.w, h: self.h, data: self.data.iter().map(|v| !v).collect() }
    }

    pub fn and(&self, o: &Mask) -> Self {
        Self { w: self.w, h: self.h, data: self.data.iter().zip(&o.data).map(|(a, b)| *a && *b).collect() }
    }

    pub fn or(&self, o: &Mask) -> Self {
        Self { w: self.w, h: self.h, data: self.data.iter().zip(&o.data).map(|(a, b)| *a || *b).collect() }
    }
}

/// cv2.getStructuringElement(MORPH_ELLIPSE, (k, k)) as offsets from the center.
pub fn ellipse_kernel(k: usize) -> Vec<(isize, isize)> {
    let r = (k / 2) as isize;
    let c = r as f64;
    let inv_r2 = if r > 0 { 1.0 / (c * c) } else { 0.0 };
    let mut out = Vec::new();
    for i in 0..k as isize {
        let dy = i - r;
        let (j1, j2) = if dy.abs() <= r && r > 0 {
            let dx = (c * (1.0 - (dy * dy) as f64 * inv_r2).max(0.0).sqrt()).round() as isize;
            ((r - dx).max(0), (r + dx + 1).min(k as isize))
        } else {
            (0, k as isize)
        };
        for j in j1..j2 {
            out.push((j - r, dy));
        }
    }
    out
}

pub fn rect_kernel(kw: usize, kh: usize) -> Vec<(isize, isize)> {
    let (rx, ry) = ((kw / 2) as isize, (kh / 2) as isize);
    let mut out = Vec::new();
    for dy in -ry..kh as isize - ry {
        for dx in -rx..kw as isize - rx {
            out.push((dx, dy));
        }
    }
    out
}

/// The kernel as runs per row: (dy, first dx, last dx).
fn kernel_runs(kernel: &[(isize, isize)]) -> Vec<(isize, isize, isize)> {
    let mut rows: Vec<isize> = kernel.iter().map(|k| k.1).collect();
    rows.sort();
    rows.dedup();
    rows.into_iter()
        .map(|dy| {
            let xs = kernel.iter().filter(|k| k.1 == dy).map(|k| k.0);
            (dy, xs.clone().min().unwrap(), xs.max().unwrap())
        })
        .collect()
}

/// Per row, how many pixels are set before each x (w + 1 entries per row).
fn row_counts(m: &Mask) -> Vec<u32> {
    let w1 = m.w + 1;
    let mut c = vec![0u32; m.h * w1];
    par_rows(&mut c, w1, |y, row| {
        for x in 0..m.w {
            row[x + 1] = row[x] + m.data[y * m.w + x] as u32;
        }
    });
    c
}

/// Erosion (all set under the kernel); outside the image counts as set, as in OpenCV.
pub fn erode(m: &Mask, kernel: &[(isize, isize)]) -> Mask {
    let runs = kernel_runs(kernel);
    let counts = row_counts(m);
    let (w, h) = (m.w as isize, m.h as isize);
    let mut data = vec![false; m.w * m.h];
    par_rows(&mut data, m.w, |y, row| {
        for (x, out) in row.iter_mut().enumerate() {
            *out = runs.iter().all(|&(dy, lo, hi)| {
                let yy = y as isize + dy;
                if yy < 0 || yy >= h {
                    return true;
                }
                let (a, b) = ((x as isize + lo).max(0), (x as isize + hi).min(w - 1));
                if a > b {
                    return true;
                }
                let r = &counts[yy as usize * (m.w + 1)..];
                (r[b as usize + 1] - r[a as usize]) as isize == b - a + 1
            });
        }
    });
    Mask { w: m.w, h: m.h, data }
}

/// Dilation (any set under the kernel, reflected); outside the image counts as unset.
pub fn dilate(m: &Mask, kernel: &[(isize, isize)]) -> Mask {
    let runs = kernel_runs(kernel);
    let counts = row_counts(m);
    let (w, h) = (m.w as isize, m.h as isize);
    let mut data = vec![false; m.w * m.h];
    par_rows(&mut data, m.w, |y, row| {
        for (x, out) in row.iter_mut().enumerate() {
            *out = runs.iter().any(|&(dy, lo, hi)| {
                let yy = y as isize - dy;
                if yy < 0 || yy >= h {
                    return false;
                }
                let (a, b) = ((x as isize - hi).max(0), (x as isize - lo).min(w - 1));
                if a > b {
                    return false;
                }
                let r = &counts[yy as usize * (m.w + 1)..];
                r[b as usize + 1] > r[a as usize]
            });
        }
    });
    Mask { w: m.w, h: m.h, data }
}

pub fn open(m: &Mask, kernel: &[(isize, isize)]) -> Mask {
    dilate(&erode(m, kernel), kernel)
}

/// Erosion of float data with a rectangle (minimum under it), BORDER_REFLECT_101
/// (cv2.erode on float images: the border is ignored, which is the same thing for a minimum).
pub fn erode_f_rows(src: &ImgF, kw: usize) -> ImgF {
    let r = (kw / 2) as isize;
    ImgF::from_fn(src.w, src.h, |x, y| {
        let mut m = f32::INFINITY;
        for dx in -r..kw as isize - r {
            let xx = x as isize + dx;
            if xx >= 0 && (xx as usize) < src.w {
                m = m.min(src.at(xx as usize, y));
            }
        }
        m
    })
}

/// Every unset region not connected to the image border, set (cv2.floodFill from outside).
pub fn fill_holes(m: &Mask) -> Mask {
    let (w, h) = (m.w, m.h);
    let mut outside = vec![false; w * h];
    let mut stack = Vec::new();
    for x in 0..w {
        stack.push((x, 0));
        stack.push((x, h - 1));
    }
    for y in 0..h {
        stack.push((0, y));
        stack.push((w - 1, y));
    }
    while let Some((x, y)) = stack.pop() {
        let i = y * w + x;
        if m.data[i] || outside[i] {
            continue;
        }
        outside[i] = true;
        if x > 0 {
            stack.push((x - 1, y));
        }
        if x + 1 < w {
            stack.push((x + 1, y));
        }
        if y > 0 {
            stack.push((x, y - 1));
        }
        if y + 1 < h {
            stack.push((x, y + 1));
        }
    }
    Mask { w, h, data: m.data.iter().zip(&outside).map(|(&s, &o)| s || !o).collect() }
}

/// Euclidean distance from each set pixel to the nearest unset one, counting
/// outside the image as unset (cv2.distanceTransform on a mask padded with 0).
/// Exact (Felzenszwalb); OpenCV's 5x5 mask is within a few percent of it.
pub fn distance_transform(m: &Mask) -> ImgF {
    let (w, h) = (m.w + 2, m.h + 2);
    let inf = 1e20f64;
    // Columns first (stored transposed, so each is a contiguous row), then rows.
    let mut cols = vec![0f64; w * h];
    par_rows(&mut cols, h, |x, col| {
        let f: Vec<f64> = (0..h)
            .map(|y| if x >= 1 && y >= 1 && x <= m.w && y <= m.h && m.at(x - 1, y - 1) { inf } else { 0.0 })
            .collect();
        edt_1d(&f, col);
    });
    let mut out = vec![0f64; w * h];
    par_rows(&mut out, w, |y, row| {
        let f: Vec<f64> = (0..w).map(|x| cols[x * h + y]).collect();
        edt_1d(&f, row);
    });
    ImgF::from_fn(m.w, m.h, |x, y| out[(y + 1) * w + x + 1].sqrt() as f32)
}

fn edt_1d(f: &[f64], d: &mut [f64]) {
    let n = f.len();
    let mut v = vec![0usize; n];
    let mut z = vec![0f64; n + 1];
    let mut k = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        let s_of = |p: usize| ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * q as f64 - 2.0 * p as f64);
        let mut s = s_of(v[k]);
        while s <= z[k] {
            k -= 1; // z[0] is -inf, so this stops at 0
            s = s_of(v[k]);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for q in 0..n {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let p = v[k];
        d[q] = (q as f64 - p as f64).powi(2) + f[p];
    }
}

// --- contours ----------------------------------------------------------------------

/// Outer outlines of the set regions (cv2.findContours RETR_EXTERNAL,
/// CHAIN_APPROX_NONE): pixel centers along each region's border.
pub fn outer_contours(m: &Mask) -> Vec<Vec<(f64, f64)>> {
    let img = GrayImage::from_raw(m.w as u32, m.h as u32, m.data.iter().map(|&v| if v { 255 } else { 0 }).collect())
        .unwrap();
    imageproc::contours::find_contours::<i32>(&img)
        .into_iter()
        .filter(|c| c.border_type == imageproc::contours::BorderType::Outer && c.parent.is_none())
        .map(|c| c.points.into_iter().map(|p| (p.x as f64, p.y as f64)).collect())
        .collect()
}

/// Corner points only (CHAIN_APPROX_SIMPLE): drops points in the middle of
/// straight horizontal, vertical or diagonal runs.
pub fn simplify_chain(c: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let n = c.len();
    if n < 3 {
        return c.to_vec();
    }
    let mut out = Vec::new();
    for i in 0..n {
        let p = c[(i + n - 1) % n];
        let q = c[i];
        let r = c[(i + 1) % n];
        if (q.0 - p.0, q.1 - p.1) != (r.0 - q.0, r.1 - q.1) {
            out.push(q);
        }
    }
    if out.is_empty() { vec![c[0]] } else { out }
}

/// cv2.contourArea: the polygon's area (shoelace).
pub fn contour_area(c: &[(f64, f64)]) -> f64 {
    let n = c.len();
    let mut s = 0.0;
    for i in 0..n {
        let (x1, y1) = c[i];
        let (x2, y2) = c[(i + 1) % n];
        s += x1 * y2 - x2 * y1;
    }
    s.abs() / 2.0
}

/// cv2.arcLength(c, closed=True).
pub fn arc_length(c: &[(f64, f64)]) -> f64 {
    let n = c.len();
    if n < 2 {
        return 0.0;
    }
    (0..n).map(|i| {
        let (a, b) = (c[i], c[(i + 1) % n]);
        (b.0 - a.0).hypot(b.1 - a.1)
    }).sum()
}

/// Centroid of the polygon (cv2.moments m10/m00, m01/m00).
pub fn contour_centroid(c: &[(f64, f64)]) -> (f64, f64) {
    let n = c.len();
    let (mut a, mut cx, mut cy) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (x1, y1) = c[i];
        let (x2, y2) = c[(i + 1) % n];
        let cross = x1 * y2 - x2 * y1;
        a += cross;
        cx += (x1 + x2) * cross;
        cy += (y1 + y2) * cross;
    }
    if a.abs() < 1e-12 {
        let k = n as f64;
        return (c.iter().map(|p| p.0).sum::<f64>() / k, c.iter().map(|p| p.1).sum::<f64>() / k);
    }
    (cx / (3.0 * a), cy / (3.0 * a))
}

/// cv2.minEnclosingCircle: (cx, cy, r), Welzl's algorithm.
pub fn min_enclosing_circle(points: &[(f64, f64)]) -> (f64, f64, f64) {
    let mut p = points.to_vec();
    // A fixed shuffle (deterministic) keeps the expected time linear.
    let mut seed = 0x9E3779B97F4A7C15u64;
    for i in (1..p.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        p.swap(i, (seed % (i as u64 + 1)) as usize);
    }
    let inside = |c: (f64, f64, f64), q: (f64, f64)| (q.0 - c.0).hypot(q.1 - c.1) <= c.2 * (1.0 + 1e-9) + 1e-9;
    let two = |a: (f64, f64), b: (f64, f64)| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0, (a.0 - b.0).hypot(a.1 - b.1) / 2.0);
    let three = |a: (f64, f64), b: (f64, f64), c: (f64, f64)| {
        let d = 2.0 * (a.0 * (b.1 - c.1) + b.0 * (c.1 - a.1) + c.0 * (a.1 - b.1));
        if d.abs() < 1e-12 {
            let mut best = two(a, b);
            for cand in [two(a, c), two(b, c)] {
                if cand.2 > best.2 {
                    best = cand;
                }
            }
            return best;
        }
        let (a2, b2, c2) = (a.0 * a.0 + a.1 * a.1, b.0 * b.0 + b.1 * b.1, c.0 * c.0 + c.1 * c.1);
        let ux = (a2 * (b.1 - c.1) + b2 * (c.1 - a.1) + c2 * (a.1 - b.1)) / d;
        let uy = (a2 * (c.0 - b.0) + b2 * (a.0 - c.0) + c2 * (b.0 - a.0)) / d;
        (ux, uy, (ux - a.0).hypot(uy - a.1))
    };
    if p.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let mut c = (p[0].0, p[0].1, 0.0);
    for i in 1..p.len() {
        if inside(c, p[i]) {
            continue;
        }
        c = (p[i].0, p[i].1, 0.0);
        for j in 0..i {
            if inside(c, p[j]) {
                continue;
            }
            c = two(p[i], p[j]);
            for k in 0..j {
                if !inside(c, p[k]) {
                    c = three(p[i], p[j], p[k]);
                }
            }
        }
    }
    c
}

/// cv2.approxPolyDP(c, eps, closed=True): Douglas-Peucker.
pub fn approx_poly(c: &[(f64, f64)], eps: f64) -> Vec<(f64, f64)> {
    let n = c.len();
    if n < 3 {
        return c.to_vec();
    }
    // Split the closed curve at the point farthest from the first.
    let far = (0..n).max_by(|&a, &b| {
        let da = (c[a].0 - c[0].0).hypot(c[a].1 - c[0].1);
        let db = (c[b].0 - c[0].0).hypot(c[b].1 - c[0].1);
        da.partial_cmp(&db).unwrap()
    }).unwrap();
    fn dp(pts: &[(f64, f64)], eps: f64, out: &mut Vec<(f64, f64)>) {
        let (a, b) = (pts[0], pts[pts.len() - 1]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = dx.hypot(dy);
        let mut best = (0, 0.0);
        for (i, p) in pts.iter().enumerate().skip(1).take(pts.len().saturating_sub(2)) {
            let d = if len < 1e-12 { (p.0 - a.0).hypot(p.1 - a.1) } else { ((p.0 - a.0) * dy - (p.1 - a.1) * dx).abs() / len };
            if d > best.1 {
                best = (i, d);
            }
        }
        if best.1 > eps {
            dp(&pts[..=best.0], eps, out);
            dp(&pts[best.0..], eps, out);
        } else {
            out.push(a);
        }
    }
    let mut out = Vec::new();
    let first: Vec<_> = c[..=far].to_vec();
    let mut second: Vec<_> = c[far..].to_vec();
    second.push(c[0]);
    dp(&first, eps, &mut out);
    dp(&second, eps, &mut out);
    out
}

// --- geometry ----------------------------------------------------------------------

/// Bilinear sample of src at (x, y), 0 outside the image (cv2.remap's
/// BORDER_CONSTANT, value 0).
#[inline]
pub fn sample(src: &ImgF, x: f32, y: f32) -> f32 {
    let (w, h) = (src.w as isize, src.h as isize);
    if !(x.is_finite() && y.is_finite()) {
        return 0.0;
    }
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (x0, y0) = (x0 as isize, y0 as isize);
    let get = |xx: isize, yy: isize| {
        if xx < 0 || yy < 0 || xx >= w || yy >= h { 0.0 } else { src.data[yy as usize * src.w + xx as usize] }
    };
    let top = get(x0, y0) * (1.0 - fx) + get(x0 + 1, y0) * fx;
    let bottom = get(x0, y0 + 1) * (1.0 - fx) + get(x0 + 1, y0 + 1) * fx;
    top * (1.0 - fy) + bottom * fy
}

/// Bilinear sample with BORDER_REPLICATE.
#[inline]
pub fn sample_replicate(src: &ImgF, x: f32, y: f32) -> f32 {
    let x = x.clamp(0.0, (src.w - 1) as f32);
    let y = y.clamp(0.0, (src.h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(src.w - 1), (y0 + 1).min(src.h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let top = src.at(x0, y0) * (1.0 - fx) + src.at(x1, y0) * fx;
    let bottom = src.at(x0, y1) * (1.0 - fx) + src.at(x1, y1) * fx;
    top * (1.0 - fy) + bottom * fy
}

/// A 2x3 affine matrix [[a, b, c], [d, e, f]]: (x, y) -> (a x + b y + c, d x + e y + f).
pub type Affine = [[f64; 3]; 2];

pub fn invert_affine(m: &Affine) -> Affine {
    let [[a, b, c], [d, e, f]] = *m;
    let det = a * e - b * d;
    let det = if det.abs() < 1e-15 { 1e-15 } else { det };
    let (ia, ib, id, ie) = (e / det, -b / det, -d / det, a / det);
    [[ia, ib, -(ia * c + ib * f)], [id, ie, -(id * c + ie * f)]]
}

/// cv2.warpAffine with INTER_LINEAR. inverse: m maps output to input
/// (WARP_INVERSE_MAP); else input to output. replicate: BORDER_REPLICATE, else 0.
pub fn warp_affine(src: &ImgF, m: &Affine, w: usize, h: usize, inverse: bool, replicate_border: bool) -> ImgF {
    let m = if inverse { *m } else { invert_affine(m) };
    ImgF::par_from_fn(w, h, |x, y| {
        let (xf, yf) = (x as f64, y as f64);
        let sx = (m[0][0] * xf + m[0][1] * yf + m[0][2]) as f32;
        let sy = (m[1][0] * xf + m[1][1] * yf + m[1][2]) as f32;
        if replicate_border { sample_replicate(src, sx, sy) } else { sample(src, sx, sy) }
    })
}

pub fn warp_affine_rgb(src: &RgbImage, m: &Affine, w: usize, h: usize, inverse: bool, replicate_border: bool) -> RgbImage {
    let ch = split(src);
    merge(&[
        warp_affine(&ch[0], m, w, h, inverse, replicate_border),
        warp_affine(&ch[1], m, w, h, inverse, replicate_border),
        warp_affine(&ch[2], m, w, h, inverse, replicate_border),
    ])
}

/// cv2.resize with INTER_LINEAR (pixel centers aligned).
pub fn resize_linear(src: &ImgF, w: usize, h: usize) -> ImgF {
    let (sx, sy) = (src.w as f64 / w as f64, src.h as f64 / h as f64);
    ImgF::from_fn(w, h, |x, y| {
        let fx = ((x as f64 + 0.5) * sx - 0.5) as f32;
        let fy = ((y as f64 + 0.5) * sy - 0.5) as f32;
        sample_replicate(src, fx, fy)
    })
}

/// cv2.resize with INTER_AREA: each output pixel is the mean of the input
/// area it covers (for shrinking; growing falls back to linear).
pub fn resize_area(src: &ImgF, w: usize, h: usize) -> ImgF {
    if w >= src.w && h >= src.h {
        return resize_linear(src, w, h);
    }
    let (sx, sy) = (src.w as f64 / w as f64, src.h as f64 / h as f64);
    // Per output column/row: the input pixels it covers, with weights.
    let spans = |n_out: usize, n_in: usize, s: f64| -> Vec<Vec<(usize, f32)>> {
        (0..n_out).map(|o| {
            let (a, b) = (o as f64 * s, ((o + 1) as f64 * s).min(n_in as f64));
            let mut v = Vec::new();
            let mut i = a.floor() as usize;
            while (i as f64) < b && i < n_in {
                let lo = (i as f64).max(a);
                let hi = ((i + 1) as f64).min(b);
                if hi > lo {
                    v.push((i, ((hi - lo) / (b - a)) as f32));
                }
                i += 1;
            }
            v
        }).collect()
    };
    let xs = spans(w, src.w, sx);
    let ys = spans(h, src.h, sy);
    let mut tmp = ImgF::new(w, src.h);
    for y in 0..src.h {
        for (x, span) in xs.iter().enumerate() {
            tmp.data[y * w + x] = span.iter().map(|&(i, k)| src.data[y * src.w + i] * k).sum();
        }
    }
    let mut out = ImgF::new(w, h);
    for (y, span) in ys.iter().enumerate() {
        for x in 0..w {
            out.data[y * w + x] = span.iter().map(|&(i, k)| tmp.data[i * w + x] * k).sum();
        }
    }
    out
}

/// cv2.resize with INTER_CUBIC (a = -0.75).
pub fn resize_cubic(src: &ImgF, w: usize, h: usize) -> ImgF {
    let (sx, sy) = (src.w as f64 / w as f64, src.h as f64 / h as f64);
    let weights = |t: f32| {
        let a = -0.75f32;
        let w0 = ((a * (t + 1.0) - 5.0 * a) * (t + 1.0) + 8.0 * a) * (t + 1.0) - 4.0 * a;
        let w1 = ((a + 2.0) * t - (a + 3.0)) * t * t + 1.0;
        let w2 = ((a + 2.0) * (1.0 - t) - (a + 3.0)) * (1.0 - t) * (1.0 - t) + 1.0;
        [w0, w1, w2, 1.0 - w0 - w1 - w2]
    };
    let cols: Vec<(isize, [f32; 4])> = (0..w).map(|x| {
        let f = (x as f64 + 0.5) * sx - 0.5;
        let i = f.floor();
        (i as isize, weights((f - i) as f32))
    }).collect();
    let mut tmp = ImgF::new(w, src.h);
    for y in 0..src.h {
        for (x, &(i, k)) in cols.iter().enumerate() {
            let mut acc = 0.0;
            for (j, kk) in k.iter().enumerate() {
                acc += kk * src.data[y * src.w + replicate(i - 1 + j as isize, src.w)];
            }
            tmp.data[y * w + x] = acc;
        }
    }
    let mut out = ImgF::new(w, h);
    for y in 0..h {
        let f = (y as f64 + 0.5) * sy - 0.5;
        let i = f.floor();
        let k = weights((f - i) as f32);
        for x in 0..w {
            let mut acc = 0.0;
            for (j, kk) in k.iter().enumerate() {
                acc += kk * tmp.data[replicate(i as isize - 1 + j as isize, src.h) * w + x];
            }
            out.data[y * w + x] = acc;
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Interp {
    Linear,
    Area,
    Cubic,
}

pub fn resize_f(src: &ImgF, w: usize, h: usize, how: Interp) -> ImgF {
    match how {
        Interp::Linear => resize_linear(src, w, h),
        Interp::Area => resize_area(src, w, h),
        Interp::Cubic => resize_cubic(src, w, h),
    }
}

pub fn resize_rgb(src: &RgbImage, w: u32, h: u32, how: Interp) -> RgbImage {
    if (src.width(), src.height()) == (w, h) {
        return src.clone();
    }
    let ch = split(src);
    let (w, h) = (w as usize, h as usize);
    merge(&[resize_f(&ch[0], w, h, how), resize_f(&ch[1], w, h, how), resize_f(&ch[2], w, h, how)])
}

pub fn resize_gray(src: &GrayImage, w: u32, h: u32, how: Interp) -> GrayImage {
    resize_f(&ImgF::from_gray(src), w as usize, h as usize, how).to_gray()
}

/// cv2.matchTemplate(image, templ, TM_CCORR_NORMED, mask): for each placement,
/// sum(T I M) / sqrt(sum((T M)^2) sum((I M)^2)) with a 0/1 mask; 0 where undefined.
pub fn match_template_ccorr_normed(image: &ImgF, templ: &ImgF, mask: &ImgF) -> ImgF {
    let (ow, oh) = (image.w - templ.w + 1, image.h - templ.h + 1);
    // Only the template pixels the mask keeps: (offset in the image, template x mask, mask).
    let used: Vec<(usize, f64, f64)> = (0..templ.data.len())
        .filter(|&k| mask.data[k] != 0.0)
        .map(|k| ((k / templ.w) * image.w + k % templ.w, (templ.data[k] * mask.data[k]) as f64, mask.data[k] as f64))
        .collect();
    let tt: f64 = used.iter().map(|u| u.1 * u.1).sum();
    let mut out = ImgF::new(ow, oh);
    par_rows_min(&mut out.data, ow, 1, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let base = y * image.w + x;
            let (mut num, mut ii) = (0f64, 0f64);
            for &(off, tm, m) in &used {
                let v = image.data[base + off] as f64 * m;
                num += tm * v;
                ii += v * v;
            }
            let den = (tt * ii).sqrt();
            *o = if den > 0.0 { (num / den) as f32 } else { 0.0 };
        }
    });
    out
}

// --- statistics ---------------------------------------------------------------------

/// numpy.percentile (linear interpolation) of the values; sorts a copy.
pub fn percentile(values: &[f32], q: f64) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    percentile_sorted(&v, q)
}

pub fn percentile_sorted(v: &[f32], q: f64) -> f32 {
    let pos = q / 100.0 * (v.len() - 1) as f64;
    let i = pos.floor() as usize;
    let f = (pos - i as f64) as f32;
    if i + 1 < v.len() { v[i] * (1.0 - f) + v[i + 1] * f } else { v[i] }
}

/// numpy.median.
pub fn median(values: &[f32]) -> f32 {
    percentile(values, 50.0)
}

pub fn median_f64(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
}

/// Least squares: x minimising |A x - b| (A: rows of n columns), by normal
/// equations with a little pivoting. Fine for the small, well-conditioned fits here.
pub fn lstsq(rows: &[Vec<f64>], b: &[f64]) -> Option<Vec<f64>> {
    let n = rows.first()?.len();
    let mut ata = vec![vec![0f64; n]; n];
    let mut atb = vec![0f64; n];
    for (r, &bi) in rows.iter().zip(b) {
        for i in 0..n {
            atb[i] += r[i] * bi;
            for j in 0..n {
                ata[i][j] += r[i] * r[j];
            }
        }
    }
    solve(ata, atb)
}

/// Solve A x = b (Gaussian elimination, partial pivoting).
pub fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for c in 0..n {
        let p = (c..n).max_by(|&i, &j| a[i][c].abs().partial_cmp(&a[j][c].abs()).unwrap())?;
        if a[p][c].abs() < 1e-12 {
            return None;
        }
        a.swap(c, p);
        b.swap(c, p);
        for r in c + 1..n {
            let f = a[r][c] / a[c][c];
            for k in c..n {
                a[r][k] -= f * a[c][k];
            }
            b[r] -= f * b[c];
        }
    }
    let mut x = vec![0f64; n];
    for r in (0..n).rev() {
        let s: f64 = (r + 1..n).map(|k| a[r][k] * x[k]).sum();
        x[r] = (b[r] - s) / a[r][r];
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    #[test]
    fn gaussian_kernels_match_opencv() {
        assert_eq!(gaussian_kernel(2.0, true).len(), 13);
        assert_eq!(gaussian_kernel(2.0, false).len(), 17);
        assert_eq!(gaussian_kernel_n(3, 0.0), vec![0.25, 0.5, 0.25]);
        let k = gaussian_kernel(1.5, false);
        assert!((k.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn blur_keeps_flat_images_flat() {
        let img = ImgF::filled(20, 10, 7.0);
        let b = gaussian_blur(&img, 3.0);
        assert!(b.data.iter().all(|&v| (v - 7.0).abs() < 1e-4));
    }

    #[test]
    fn otsu_splits_two_levels() {
        let g = GrayImage::from_fn(10, 10, |x, _| Luma([if x < 5 { 40 } else { 200 }]));
        let t = otsu_u8(&g);
        assert!((40..200).contains(&t));
    }

    #[test]
    fn distance_transform_of_a_disc() {
        let m = Mask::from_fn(101, 101, |x, y| ((x as f64 - 50.0).hypot(y as f64 - 50.0)) < 40.0);
        let d = distance_transform(&m);
        assert!((d.at(50, 50) - 40.0).abs() < 1.5, "{}", d.at(50, 50));
    }

    #[test]
    fn contours_of_a_square() {
        let m = Mask::from_fn(20, 20, |x, y| (5..15).contains(&x) && (5..15).contains(&y));
        let c = outer_contours(&m);
        assert_eq!(c.len(), 1);
        assert!((contour_area(&c[0]) - 81.0).abs() < 1e-9);
        assert!((arc_length(&c[0]) - 36.0).abs() < 1e-9);
        let (cx, cy) = contour_centroid(&c[0]);
        assert!((cx - 9.5).abs() < 1e-9 && (cy - 9.5).abs() < 1e-9);
        assert_eq!(simplify_chain(&c[0]).len(), 4);
    }

    #[test]
    fn enclosing_circle() {
        let pts = [(0.0, 0.0), (10.0, 0.0), (5.0, 3.0), (5.0, -5.0)];
        let (x, y, r) = min_enclosing_circle(&pts);
        for p in pts {
            assert!((p.0 - x).hypot(p.1 - y) <= r + 1e-6);
        }
        assert!((r - 5.0).abs() < 0.2, "{r}");
    }

    #[test]
    fn median_blur_removes_specks() {
        let mut g = GrayImage::from_pixel(9, 9, Luma([100]));
        g.put_pixel(4, 4, Luma([255]));
        assert_eq!(median_blur(&g, 3).get_pixel(4, 4).0[0], 100);
    }

    #[test]
    fn area_resize_averages() {
        let img = ImgF::from_fn(4, 2, |x, _| x as f32);
        let r = resize_area(&img, 2, 1);
        assert_eq!(r.data, vec![0.5, 2.5]);
    }

    #[test]
    fn lab_of_white_and_black() {
        let img = RgbImage::from_fn(2, 1, |x, _| if x == 0 { Rgb([255, 255, 255]) } else { Rgb([0, 0, 0]) });
        let lab = to_lab(&img);
        assert_eq!((lab[0].data[0], lab[1].data[0], lab[2].data[0]), (255.0, 128.0, 128.0));
        assert_eq!((lab[0].data[1], lab[1].data[1], lab[2].data[1]), (0.0, 128.0, 128.0));
    }

    #[test]
    fn template_matches_itself() {
        let img = ImgF::from_fn(30, 30, |x, y| (((x * 7919 + y * 104729) ^ (x * y * 31)) % 251) as f32);
        let t = img.crop(10, 8, 8, 8);
        let m = ImgF::filled(8, 8, 1.0);
        let r = match_template_ccorr_normed(&img, &t, &m);
        assert_eq!(r.argmax(), (10, 8));
    }

    #[test]
    fn least_squares_line() {
        let rows: Vec<Vec<f64>> = (0..10).map(|i| vec![1.0, i as f64]).collect();
        let b: Vec<f64> = (0..10).map(|i| 3.0 + 2.0 * i as f64).collect();
        let x = lstsq(&rows, &b).unwrap();
        assert!((x[0] - 3.0).abs() < 1e-9 && (x[1] - 2.0).abs() < 1e-9);
    }
}
