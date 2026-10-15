//! Stitching: join overlapping images into one large field of view.
//!
//! Corners (FAST) with oriented binary descriptors (as ORB) are matched
//! between every pair of images; RANSAC finds how each pair fits:
//! - "scans" (default): a flat sample moved under the camera, the microscope
//!   case: shift, rotation and a little scale (a similarity transform).
//! - "panorama": a camera turned to look around: perspective (a homography).
//! Each image is placed via the strongest chain of pairs to a reference image
//! and blended with weights that fall off towards its edges.

use image::{Rgb, RgbImage};

use crate::imgproc::{self as ip, ImgF};

const WORK_SIZE: f64 = 900.0; // features are found on copies at most this big
const PATCH: f64 = 15.0;
const MIN_INLIERS: usize = 12;

const NO_OVERLAP: &str = "Couldn't find enough overlap between the images. Neighbouring images \
    should overlap by about a third and show some detail (not blank areas).";
const NO_FIT: &str = "Couldn't work out how the images fit together. Try more overlap, or the other mode.";

type H = [[f64; 3]; 3];

fn mul(a: &H, b: &H) -> H {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

fn inv(m: &H) -> Option<H> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let c = |r1: usize, c1: usize, r2: usize, c2: usize| m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1];
    Some([
        [c(1, 1, 2, 2) / det, -c(0, 1, 2, 2) / det, c(0, 1, 1, 2) / det],
        [-c(1, 0, 2, 2) / det, c(0, 0, 2, 2) / det, -c(0, 0, 1, 2) / det],
        [c(1, 0, 2, 1) / det, -c(0, 0, 2, 1) / det, c(0, 0, 1, 1) / det],
    ])
}

fn apply(m: &H, p: (f64, f64)) -> (f64, f64) {
    let w = m[2][0] * p.0 + m[2][1] * p.1 + m[2][2];
    ((m[0][0] * p.0 + m[0][1] * p.1 + m[0][2]) / w, (m[1][0] * p.0 + m[1][1] * p.1 + m[1][2]) / w)
}

// --- features ------------------------------------------------------------------------

struct Feature {
    x: f64,
    y: f64,
    desc: [u64; 4],
}

const CIRCLE: [(isize, isize); 16] = [
    (0, -3), (1, -3), (2, -2), (3, -1), (3, 0), (3, 1), (2, 2), (1, 3),
    (0, 3), (-1, 3), (-2, 2), (-3, 1), (-3, 0), (-3, -1), (-2, -2), (-1, -3),
];

fn fast_corners(g: &ImgF, threshold: f32) -> Vec<(usize, usize, f32)> {
    let (w, h) = (g.w, g.h);
    let b = PATCH as usize + 4;
    if w <= 2 * b || h <= 2 * b {
        return Vec::new();
    }
    let mut score = ImgF::new(w, h);
    for y in b..h - b {
        for x in b..w - b {
            let c = g.at(x, y);
            let ring: Vec<f32> = CIRCLE.iter().map(|&(dx, dy)| g.at((x as isize + dx) as usize, (y as isize + dy) as usize)).collect();
            // 9 contiguous all brighter or all darker.
            for sign in [1.0f32, -1.0] {
                let mut run = 0;
                let mut best = 0;
                for k in 0..32 {
                    if sign * (ring[k % 16] - c) > threshold {
                        run += 1;
                        best = best.max(run);
                    } else {
                        run = 0;
                    }
                }
                if best >= 9 {
                    let s: f32 = ring.iter().map(|&v| ((v - c).abs() - threshold).max(0.0)).sum();
                    score.set(x, y, score.at(x, y).max(s));
                }
            }
        }
    }
    let mut out = Vec::new();
    for y in b..h - b {
        for x in b..w - b {
            let s = score.at(x, y);
            if s > 0.0 && (-1..=1).all(|dy: isize| (-1..=1).all(|dx: isize| {
                (dx == 0 && dy == 0) || score.at((x as isize + dx) as usize, (y as isize + dy) as usize) <= s
            })) {
                out.push((x, y, s));
            }
        }
    }
    out
}

fn pattern() -> Vec<((f64, f64), (f64, f64))> {
    // A fixed pseudo-random set of point pairs in the patch (Gaussian-ish).
    let mut seed = 0x2545F4914F6CDD1Du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let a = (seed >> 11) as f64 / (1u64 << 53) as f64;
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let b = (seed >> 11) as f64 / (1u64 << 53) as f64;
        let n = (-2.0 * a.max(1e-12).ln()).sqrt() * (2.0 * std::f64::consts::PI * b).cos();
        (n * PATCH / 3.0).clamp(-PATCH + 2.0, PATCH - 2.0)
    };
    (0..256).map(|_| ((next(), next()), (next(), next()))).collect()
}

fn features(img: &RgbImage) -> (Vec<Feature>, f64) {
    let (w, h) = img.dimensions();
    let scale = (WORK_SIZE / w.max(h) as f64).min(1.0);
    let g = ImgF::from_gray(&ip::to_gray(img));
    let g = if scale < 1.0 { ip::resize_area(&g, (w as f64 * scale).round() as usize, (h as f64 * scale).round() as usize) } else { g };
    let smooth = ip::gaussian_blur(&g, 1.2);
    let mut corners = Vec::new();
    for t in [20.0, 10.0, 5.0, 2.5] {
        corners = fast_corners(&smooth, t);
        if corners.len() >= 300 {
            break;
        }
    }
    corners.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    corners.truncate(2000);
    let pat = pattern();
    let feats = corners
        .into_iter()
        .map(|(x, y, _)| {
            // Orientation: intensity centroid of the patch.
            let (mut m01, mut m10) = (0.0, 0.0);
            let r = PATCH as isize;
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx * dx + dy * dy <= r * r {
                        let v = smooth.at((x as isize + dx) as usize, (y as isize + dy) as usize) as f64;
                        m10 += dx as f64 * v;
                        m01 += dy as f64 * v;
                    }
                }
            }
            let a = m01.atan2(m10);
            let (c, s) = (a.cos(), a.sin());
            let mut desc = [0u64; 4];
            for (k, ((x1, y1), (x2, y2))) in pat.iter().enumerate() {
                let p = |px: f64, py: f64| ip::sample(&smooth, (x as f64 + c * px - s * py) as f32, (y as f64 + s * px + c * py) as f32);
                if p(*x1, *y1) < p(*x2, *y2) {
                    desc[k / 64] |= 1 << (k % 64);
                }
            }
            Feature { x: x as f64, y: y as f64, desc }
        })
        .collect();
    (feats, scale)
}

fn hamming(a: &[u64; 4], b: &[u64; 4]) -> u32 {
    (0..4).map(|i| (a[i] ^ b[i]).count_ones()).sum()
}

fn best_two(d: &[u64; 4], others: &[Feature]) -> (usize, u32, u32) {
    let (mut bi, mut b1, mut b2) = (0, u32::MAX, u32::MAX);
    for (i, f) in others.iter().enumerate() {
        let h = hamming(d, &f.desc);
        if h < b1 {
            b2 = b1;
            b1 = h;
            bi = i;
        } else if h < b2 {
            b2 = h;
        }
    }
    (bi, b1, b2)
}

fn matches(a: &[Feature], b: &[Feature]) -> Vec<((f64, f64), (f64, f64))> {
    let back: Vec<usize> = b.iter().map(|f| best_two(&f.desc, a).0).collect();
    a.iter()
        .enumerate()
        .filter_map(|(i, f)| {
            let (j, d1, d2) = best_two(&f.desc, b);
            (d1 < 64 && (d1 as f64) < 0.8 * d2 as f64 && back[j] == i).then(|| ((f.x, f.y), (b[j].x, b[j].y)))
        })
        .collect()
}

// --- models ---------------------------------------------------------------------------

/// Least-squares similarity x' = a x - b y + tx, y' = b x + a y + ty.
fn fit_similarity(pairs: &[((f64, f64), (f64, f64))]) -> Option<H> {
    let mut rows = Vec::new();
    let mut rhs = Vec::new();
    for &((x, y), (u, v)) in pairs {
        rows.push(vec![x, -y, 1.0, 0.0]);
        rhs.push(u);
        rows.push(vec![y, x, 0.0, 1.0]);
        rhs.push(v);
    }
    let s = ip::lstsq(&rows, &rhs)?;
    Some([[s[0], -s[1], s[2]], [s[1], s[0], s[3]], [0.0, 0.0, 1.0]])
}

fn normalizer(pts: &[(f64, f64)]) -> H {
    let n = pts.len() as f64;
    let (mx, my) = (pts.iter().map(|p| p.0).sum::<f64>() / n, pts.iter().map(|p| p.1).sum::<f64>() / n);
    let d = pts.iter().map(|p| (p.0 - mx).hypot(p.1 - my)).sum::<f64>() / n;
    let s = if d > 1e-9 { 2f64.sqrt() / d } else { 1.0 };
    [[s, 0.0, -s * mx], [0.0, s, -s * my], [0.0, 0.0, 1.0]]
}

/// Homography by DLT (h33 = 1) on normalised points.
fn fit_homography(pairs: &[((f64, f64), (f64, f64))]) -> Option<H> {
    let ta = normalizer(&pairs.iter().map(|p| p.0).collect::<Vec<_>>());
    let tb = normalizer(&pairs.iter().map(|p| p.1).collect::<Vec<_>>());
    let mut rows = Vec::new();
    let mut rhs = Vec::new();
    for &(p, q) in pairs {
        let (x, y) = apply(&ta, p);
        let (u, v) = apply(&tb, q);
        rows.push(vec![x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y]);
        rhs.push(u);
        rows.push(vec![0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y]);
        rhs.push(v);
    }
    let s = ip::lstsq(&rows, &rhs)?;
    let hn = [[s[0], s[1], s[2]], [s[3], s[4], s[5]], [s[6], s[7], 1.0]];
    Some(mul(&inv(&tb)?, &mul(&hn, &ta)))
}

fn ransac(pairs: &[((f64, f64), (f64, f64))], panorama: bool) -> Option<(H, usize)> {
    let need = if panorama { 4 } else { 2 };
    if pairs.len() < need.max(MIN_INLIERS) {
        return None;
    }
    let fit = |s: &[((f64, f64), (f64, f64))]| if panorama { fit_homography(s) } else { fit_similarity(s) };
    let inliers = |m: &H| -> Vec<usize> {
        (0..pairs.len()).filter(|&i| {
            let (u, v) = apply(m, pairs[i].0);
            (u - pairs[i].1 .0).hypot(v - pairs[i].1 .1) < 3.0
        }).collect()
    };
    let mut seed = 12345u64;
    let mut rand = |n: usize| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((seed >> 33) as usize) % n
    };
    let mut best: Vec<usize> = Vec::new();
    for _ in 0..2000 {
        let mut idx: Vec<usize> = Vec::new();
        while idx.len() < need {
            let k = rand(pairs.len());
            if !idx.contains(&k) {
                idx.push(k);
            }
        }
        let sample: Vec<_> = idx.iter().map(|&i| pairs[i]).collect();
        if let Some(m) = fit(&sample) {
            let inl = inliers(&m);
            if inl.len() > best.len() {
                best = inl;
            }
        }
    }
    if best.len() < MIN_INLIERS {
        return None;
    }
    let m = fit(&best.iter().map(|&i| pairs[i]).collect::<Vec<_>>())?;
    let inl = inliers(&m);
    Some((m, inl.len()))
}

// --- stitching -------------------------------------------------------------------------

/// Cut off the black border around a stitched image.
pub fn crop_to_content(img: &RgbImage) -> RgbImage {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in img.enumerate_pixels() {
        if p.0.iter().any(|&v| v > 0) {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x0 == u32::MAX {
        return img.clone();
    }
    image::imageops::crop_imm(img, x0, y0, x1 - x0 + 1, y1 - y0 + 1).to_image()
}

pub fn stitch(images: &[RgbImage], mode: &str, crop: bool) -> Result<RgbImage, String> {
    let n = images.len();
    if n < 2 {
        return Err("Stitching needs at least 2 images.".into());
    }
    let panorama = mode == "panorama";
    let feats: Vec<(Vec<Feature>, f64)> = std::thread::scope(|s| {
        let hs: Vec<_> = images.iter().map(|im| s.spawn(move || features(im))).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    // How each pair fits, at full size: edges[i][j] = (map from i to j, inliers).
    let mut edges: Vec<Vec<Option<(H, usize)>>> = vec![vec![None; n]; n];
    for i in 0..n {
        for j in i + 1..n {
            let pairs = matches(&feats[i].0, &feats[j].0);
            if let Some((m, inl)) = ransac(&pairs, panorama) {
                let (si, sj) = (feats[i].1, feats[j].1);
                let to_small: H = [[si, 0.0, 0.0], [0.0, si, 0.0], [0.0, 0.0, 1.0]];
                let from_small: H = [[1.0 / sj, 0.0, 0.0], [0.0, 1.0 / sj, 0.0], [0.0, 0.0, 1.0]];
                let full = mul(&from_small, &mul(&m, &to_small));
                if let Some(back) = inv(&full) {
                    edges[i][j] = Some((full, inl));
                    edges[j][i] = Some((back, inl));
                }
            }
        }
    }
    // Reference: the best connected image; then the strongest links outwards (Prim).
    let reference = (0..n).max_by_key(|&i| edges[i].iter().flatten().map(|e| e.1).sum::<usize>()).unwrap();
    let mut to_ref: Vec<Option<H>> = vec![None; n];
    to_ref[reference] = Some([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    loop {
        let mut best: Option<(usize, usize, usize)> = None; // inliers, from (placed), to (new)
        for a in 0..n {
            if to_ref[a].is_none() {
                continue;
            }
            for b in 0..n {
                if to_ref[b].is_some() {
                    continue;
                }
                if let Some((_, inl)) = edges[b][a] {
                    if best.is_none_or(|bb| inl > bb.0) {
                        best = Some((inl, a, b));
                    }
                }
            }
        }
        let Some((_, a, b)) = best else { break };
        to_ref[b] = Some(mul(to_ref[a].as_ref().unwrap(), &edges[b][a].unwrap().0));
    }
    if to_ref.iter().any(|t| t.is_none()) {
        return Err(NO_OVERLAP.into());
    }
    let to_ref: Vec<H> = to_ref.into_iter().map(|t| t.unwrap()).collect();
    // Canvas.
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for (im, t) in images.iter().zip(&to_ref) {
        let (w, h) = (im.width() as f64, im.height() as f64);
        for c in [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)] {
            let (x, y) = apply(t, c);
            if !x.is_finite() || !y.is_finite() {
                return Err(NO_FIT.into());
            }
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    let (cw, ch) = ((x1 - x0).ceil() as usize, (y1 - y0).ceil() as usize);
    let total: usize = images.iter().map(|i| (i.width() * i.height()) as usize).sum();
    if cw == 0 || ch == 0 || cw * ch > 4 * total.max(1) || cw > 30000 || ch > 30000 {
        return Err(NO_FIT.into());
    }
    let shift: H = [[1.0, 0.0, -x0], [0.0, 1.0, -y0], [0.0, 0.0, 1.0]];
    let placed: Vec<(H, H)> = to_ref.iter().map(|t| {
        let m = mul(&shift, t);
        (m, inv(&m).unwrap())
    }).collect();
    let chans: Vec<[ImgF; 3]> = images.iter().map(ip::split).collect();
    let rows: Vec<Vec<[f32; 3]>> = std::thread::scope(|s| {
        let workers = 8.min(ch);
        let hs: Vec<_> = (0..workers).map(|k| {
            let (placed, chans, images) = (&placed, &chans, images);
            s.spawn(move || {
                let mut out = Vec::new();
                for y in (k..ch).step_by(workers) {
                    let mut row = vec![[0f32; 3]; cw];
                    let mut wsum = vec![0f32; cw];
                    for (i, (_, back)) in placed.iter().enumerate() {
                        let (iw, ih) = (images[i].width() as f64, images[i].height() as f64);
                        for x in 0..cw {
                            let (sx, sy) = apply(back, (x as f64 + 0.5, y as f64 + 0.5));
                            let (sx, sy) = (sx - 0.5, sy - 0.5);
                            if sx < 0.0 || sy < 0.0 || sx > iw - 1.0 || sy > ih - 1.0 {
                                continue;
                            }
                            let wgt = ((sx.min(iw - 1.0 - sx) + 1.0) * (sy.min(ih - 1.0 - sy) + 1.0)) as f32;
                            for c in 0..3 {
                                row[x][c] += wgt * ip::sample(&chans[i][c], sx as f32, sy as f32);
                            }
                            wsum[x] += wgt;
                        }
                    }
                    for x in 0..cw {
                        if wsum[x] > 0.0 {
                            for c in 0..3 {
                                row[x][c] /= wsum[x];
                            }
                        }
                    }
                    out.push((y, row));
                }
                out
            })
        }).collect();
        let mut all = vec![Vec::new(); ch];
        for h in hs {
            for (y, row) in h.join().unwrap() {
                all[y] = row;
            }
        }
        all
    });
    let out = RgbImage::from_fn(cw as u32, ch as u32, |x, y| {
        let p = rows[y as usize][x as usize];
        Rgb([ip::sat_u8(p[0]), ip::sat_u8(p[1]), ip::sat_u8(p[2])])
    });
    Ok(if crop { crop_to_content(&out) } else { out })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw;

    #[test]
    fn stitches_three_tiles_back() {
        // A detailed picture cut into three overlapping tiles.
        let mut seed = 1u64;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as f64 / (1u64 << 31) as f64
        };
        let noise = ImgF::from_fn(700, 300, |_, _| 0.0);
        let mut noise = noise;
        noise.data.iter_mut().for_each(|v| *v = (rnd() * 255.0) as f32);
        let noise = ip::gaussian_blur(&noise, 1.5);
        let (lo, hi) = noise.data.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), &v| (a.min(v), b.max(v)));
        let mut big = RgbImage::from_fn(700, 300, |x, y| {
            let v = ip::sat_u8((noise.at(x as usize, y as usize) - lo) / (hi - lo) * 255.0);
            Rgb([v, v, v])
        });
        for _ in 0..40 {
            let (x, y, r) = ((rnd() * 700.0) as i64, (rnd() * 300.0) as i64, 5 + (rnd() * 20.0) as i64);
            draw::fill_circle_px(&mut big, x, y, r, [(rnd() * 255.0) as u8, (rnd() * 255.0) as u8, (rnd() * 255.0) as u8]);
        }
        let tiles: Vec<RgbImage> = [0, 200, 400].iter().map(|&x0| image::imageops::crop_imm(&big, x0, 0, 300, 300).to_image()).collect();
        let result = stitch(&tiles, "scans", true).unwrap();
        assert!((result.width() as i64 - 700).abs() < 40 && (result.height() as i64 - 300).abs() < 40,
                "{}x{}", result.width(), result.height());
    }

    #[test]
    fn featureless_images_fail_clearly() {
        let blank = RgbImage::from_pixel(200, 200, Rgb([128, 128, 128]));
        assert_eq!(stitch(&[blank.clone(), blank], "scans", true).unwrap_err(), NO_OVERLAP);
    }
}
