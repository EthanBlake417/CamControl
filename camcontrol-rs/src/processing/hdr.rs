//! HDR by exposure fusion (Mertens et al.), as OpenCV's MergeMertens with its
//! defaults (contrast and saturation weights 1, well-exposedness 0), with
//! optional AlignMTB shift alignment first.

use image::RgbImage;

use crate::imgproc::{self as ip, ImgF};
use crate::processing::common::check_same_size;

pub fn exposure_fusion(images: &[RgbImage], align: bool) -> Result<RgbImage, String> {
    if images.len() < 2 {
        return Err("HDR needs at least 2 images at different exposures.".into());
    }
    check_same_size(images)?;
    let images = if align { align_mtb(images) } else { images.to_vec() };
    let fused = merge_mertens(&images);
    Ok(ip::merge(&[fused[0].map(|v| (v * 255.0 + 0.5).floor()), fused[1].map(|v| (v * 255.0 + 0.5).floor()),
                   fused[2].map(|v| (v * 255.0 + 0.5).floor())]))
}

// --- AlignMTB ------------------------------------------------------------------------

const MAX_BITS: usize = 6;
const EXCLUDE_RANGE: f32 = 4.0;

fn shifted_mask(m: &[bool], w: usize, h: usize, dx: i64, dy: i64) -> Vec<bool> {
    let mut out = vec![false; w * h];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let (sx, sy) = (x - dx, y - dy);
            if sx >= 0 && sy >= 0 && sx < w as i64 && sy < h as i64 {
                out[(y * w as i64 + x) as usize] = m[(sy * w as i64 + sx) as usize];
            }
        }
    }
    out
}

fn bitmaps(img: &ImgF) -> (Vec<bool>, Vec<bool>) {
    let mut sorted: Vec<u8> = img.data.iter().map(|&x| ip::sat_u8(x)).collect();
    sorted.sort_unstable();
    let median = sorted[sorted.len() / 2] as f32;
    let tb = img.data.iter().map(|&x| ip::sat_u8(x) as f32 > median).collect();
    let eb = img.data.iter().map(|&x| (ip::sat_u8(x) as f32 - median).abs() > EXCLUDE_RANGE).collect();
    (tb, eb)
}

fn pyramid(img: &ImgF, levels: usize) -> Vec<ImgF> {
    let mut p = vec![img.clone()];
    for _ in 0..levels {
        let last = p.last().unwrap();
        let (w, h) = ((last.w / 2).max(1), (last.h / 2).max(1));
        p.push(ImgF::from_gray(&ip::resize_linear(last, w, h).to_gray()));
    }
    p
}

fn calculate_shift(a: &ImgF, b: &ImgF) -> (i64, i64) {
    let max_level = (((a.w.max(a.h)) as f64).log2() as usize).saturating_sub(1).min(MAX_BITS - 1);
    let (pa, pb) = (pyramid(a, max_level), pyramid(b, max_level));
    let mut shift = (0i64, 0i64);
    for level in (0..=max_level).rev() {
        shift = (shift.0 * 2, shift.1 * 2);
        let (w, h) = (pa[level].w, pa[level].h);
        let (tb1, eb1) = bitmaps(&pa[level]);
        let (tb2, eb2) = bitmaps(&pb[level]);
        let mut best = (usize::MAX, shift);
        for i in -1..=1 {
            for j in -1..=1 {
                let t = (shift.0 + i, shift.1 + j);
                let st = shifted_mask(&tb2, w, h, t.0, t.1);
                let se = shifted_mask(&eb2, w, h, t.0, t.1);
                let err = (0..w * h).filter(|&k| (tb1[k] ^ st[k]) && eb1[k] && se[k]).count();
                if err < best.0 {
                    best = (err, t);
                }
            }
        }
        shift = best.1;
    }
    shift
}

/// Each image shifted (black filled) to line up with the middle one.
pub fn align_mtb(images: &[RgbImage]) -> Vec<RgbImage> {
    let pivot = images.len() / 2;
    let gray: Vec<ImgF> = images.iter().map(|i| ImgF::from_gray(&ip::to_gray(i))).collect();
    images
        .iter()
        .enumerate()
        .map(|(k, im)| {
            if k == pivot {
                return im.clone();
            }
            let (dx, dy) = calculate_shift(&gray[pivot], &gray[k]);
            let (w, h) = im.dimensions();
            RgbImage::from_fn(w, h, |x, y| {
                let (sx, sy) = (x as i64 - dx, y as i64 - dy);
                if sx >= 0 && sy >= 0 && sx < w as i64 && sy < h as i64 { *im.get_pixel(sx as u32, sy as u32) } else { image::Rgb([0, 0, 0]) }
            })
        })
        .collect()
}

// --- Mertens --------------------------------------------------------------------------

const PYR_K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

fn pyr_down(src: &ImgF) -> ImgF {
    let blurred = ip::sep_filter(src, &PYR_K, &PYR_K);
    let (w, h) = (src.w.div_ceil(2), src.h.div_ceil(2));
    ImgF::from_fn(w, h, |x, y| blurred.at((2 * x).min(src.w - 1), (2 * y).min(src.h - 1)))
}

fn pyr_up(src: &ImgF, w: usize, h: usize) -> ImgF {
    let mut up = ImgF::new(w, h);
    for y in 0..src.h {
        for x in 0..src.w {
            if 2 * x < w && 2 * y < h {
                up.set(2 * x, 2 * y, src.at(x, y) * 4.0);
            }
        }
    }
    ip::sep_filter(&up, &PYR_K, &PYR_K)
}

fn build_pyr(img: &ImgF, levels: usize) -> Vec<ImgF> {
    let mut p = vec![img.clone()];
    for _ in 0..levels {
        let next = pyr_down(p.last().unwrap());
        p.push(next);
    }
    p
}

/// cv::MergeMertens::process: fused image, float, about 0-1, channels R, G, B.
pub fn merge_mertens(images: &[RgbImage]) -> [ImgF; 3] {
    let (w, h) = (images[0].width() as usize, images[0].height() as usize);
    let chans: Vec<[ImgF; 3]> = images.iter().map(|im| {
        let c = ip::split(im);
        [c[0].map(|v| v / 255.0), c[1].map(|v| v / 255.0), c[2].map(|v| v / 255.0)]
    }).collect();
    let mut weights: Vec<ImgF> = chans.iter().map(|c| {
        let gray = ImgF::from_fn(w, h, |x, y| 0.299 * c[0].at(x, y) + 0.587 * c[1].at(x, y) + 0.114 * c[2].at(x, y));
        // Laplacian with ksize 1: [0 1 0; 1 -4 1; 0 1 0]
        let contrast = ImgF::from_fn(w, h, |x, y| {
            let p = |dx: isize, dy: isize| gray.at(ip::reflect101(x as isize + dx, w), ip::reflect101(y as isize + dy, h));
            (p(-1, 0) + p(1, 0) + p(0, -1) + p(0, 1) - 4.0 * p(0, 0)).abs()
        });
        ImgF::from_fn(w, h, |x, y| {
            let v = [c[0].at(x, y), c[1].at(x, y), c[2].at(x, y)];
            let mean = (v[0] + v[1] + v[2]) / 3.0;
            let sat = v.iter().map(|a| (a - mean) * (a - mean)).sum::<f32>().sqrt();
            contrast.at(x, y) * sat + 1e-12
        })
    }).collect();
    let mut sum = ImgF::new(w, h);
    for wt in &weights {
        sum = sum.zip(wt, |a, b| a + b);
    }
    let levels = ((w.min(h)) as f64).log2() as usize;
    let mut res: Vec<[ImgF; 3]> = Vec::new();
    for (i, c) in chans.iter().enumerate() {
        weights[i] = weights[i].zip(&sum, |a, b| a / b);
        let wp = build_pyr(&weights[i], levels);
        for ch in 0..3 {
            let mut ipyr = build_pyr(&c[ch], levels);
            for l in 0..levels {
                let up = pyr_up(&ipyr[l + 1], ipyr[l].w, ipyr[l].h);
                ipyr[l] = ipyr[l].zip(&up, |a, b| a - b);
            }
            for l in 0..=levels {
                let term = ipyr[l].zip(&wp[l], |a, b| a * b);
                if res.len() <= l {
                    res.push([ImgF::new(term.w, term.h), ImgF::new(term.w, term.h), ImgF::new(term.w, term.h)]);
                }
                res[l][ch] = res[l][ch].zip(&term, |a, b| a + b);
            }
        }
    }
    for l in (1..=levels).rev() {
        for ch in 0..3 {
            let up = pyr_up(&res[l][ch], res[l - 1][ch].w, res[l - 1][ch].h);
            res[l - 1][ch] = res[l - 1][ch].zip(&up, |a, b| a + b);
        }
    }
    res.swap_remove(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn fuses_a_wide_range() {
        let scene = |x: u32| 4.0 * x as f32 / 299.0;
        let exposures: Vec<RgbImage> = [0.25f32, 1.0, 4.0].iter().map(|&k| {
            RgbImage::from_fn(300, 200, |x, _| { let v = ip::sat_u8((scene(x) * k * 255.0).min(255.0)); Rgb([v, v, v]) })
        }).collect();
        let fused = exposure_fusion(&exposures, false).unwrap();
        let (a, b) = (fused.get_pixel(0, 100).0[0] as i32, fused.get_pixel(299, 100).0[0] as i32);
        assert!(b > a + 100, "{a} {b}");
    }

    #[test]
    fn mtb_finds_a_shift() {
        let mut noise = ImgF::new(960, 540);
        let mut seed = 7u32;
        for v in noise.data.iter_mut() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            *v = (seed >> 24) as f32;
        }
        let noise = ip::gaussian_blur(&noise, 2.0);
        let (lo, hi) = noise.data.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), &v| (a.min(v), b.max(v)));
        let base = RgbImage::from_fn(960, 540, |x, y| { let v = ip::sat_u8((noise.at(x as usize, y as usize) - lo) / (hi - lo) * 255.0); Rgb([v, v, v]) });
        let moved = RgbImage::from_fn(960, 540, |x, y| *base.get_pixel(x.saturating_sub(4), y.saturating_sub(3)));
        let (dx, dy) = calculate_shift(&ImgF::from_gray(&ip::to_gray(&base)), &ImgF::from_gray(&ip::to_gray(&moved)));
        assert_eq!((dx, dy), (-4, -3));
    }
}
