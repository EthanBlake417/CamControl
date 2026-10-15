//! Helpers shared by the processing tools: size checks and alignment.

use image::RgbImage;

use crate::imgproc::{self as ip, Affine, ImgF};

const ALIGN_WIDTH: f64 = 1000.0; // alignment is worked out on a copy about this wide

pub fn check_same_size(images: &[RgbImage]) -> Result<(), String> {
    let mut sizes: Vec<(u32, u32)> = images.iter().map(|i| (i.width(), i.height())).collect();
    sizes.sort();
    sizes.dedup();
    if sizes.len() > 1 {
        let listed: Vec<String> = sizes.iter().map(|(w, h)| format!("{w}x{h}")).collect();
        return Err(format!("All images must be the same size (got {}).", listed.join(", ")));
    }
    Ok(())
}

pub fn gray_f(img: &RgbImage) -> ImgF {
    ImgF::from_gray(&ip::to_gray(img))
}

/// Shift/rotate/scale image so it lines up with reference (ECC, affine), as
/// cv2.findTransformECC does. Unchanged if alignment can't be found.
pub fn align_to(reference: &RgbImage, image: &RgbImage) -> RgbImage {
    let (w, h) = (reference.width() as usize, reference.height() as usize);
    let scale = (ALIGN_WIDTH / w as f64).min(1.0);
    let (sw, sh) = (((w as f64 * scale).round() as usize).max(1), ((h as f64 * scale).round() as usize).max(1));
    let prep = |im: &RgbImage| ip::resize_area(&gray_f(im), sw, sh);
    let Some(mut warp) = find_transform_ecc(&prep(reference), &prep(image), 100, 1e-5) else {
        return image.clone(); // didn't converge (very blurry or featureless)
    };
    warp[0][2] /= scale;
    warp[1][2] /= scale;
    ip::warp_affine_rgb(image, &warp, w, h, true, true)
}

/// Every image aligned to the middle one.
pub fn align_all(images: &[RgbImage]) -> Vec<RgbImage> {
    let r = images.len() / 2;
    images.iter().enumerate().map(|(i, im)| if i == r { im.clone() } else { align_to(&images[r], im) }).collect()
}

/// ECC image alignment (Evangelidis & Psarakis), affine model: the warp
/// mapping template pixels to input pixels.
pub fn find_transform_ecc(template: &ImgF, input: &ImgF, iterations: usize, eps: f64) -> Option<Affine> {
    let k = ip::gaussian_kernel_n(5, 0.0);
    let t = ip::sep_filter(template, &k, &k);
    let im = ip::sep_filter(input, &k, &k);
    let d = [-0.5f32, 0.0, 0.5];
    let one = [1.0f32];
    let gx = ip::sep_filter(&im, &d, &one);
    let gy = ip::sep_filter(&im, &one, &d);
    let (w, h) = (t.w, t.h);
    let mut warp: Affine = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let mut last_rho = f64::NEG_INFINITY;
    for _ in 0..iterations {
        // Warp the input and its gradients into the template's frame.
        let n = w * h;
        let mut iw = vec![0f64; n];
        let mut gxw = vec![0f64; n];
        let mut gyw = vec![0f64; n];
        let mut valid = vec![false; n];
        for y in 0..h {
            for x in 0..w {
                let sx = warp[0][0] * x as f64 + warp[0][1] * y as f64 + warp[0][2];
                let sy = warp[1][0] * x as f64 + warp[1][1] * y as f64 + warp[1][2];
                if sx < 0.0 || sy < 0.0 || sx > (im.w - 1) as f64 || sy > (im.h - 1) as f64 {
                    continue;
                }
                let i = y * w + x;
                valid[i] = true;
                iw[i] = ip::sample(&im, sx as f32, sy as f32) as f64;
                gxw[i] = ip::sample(&gx, sx as f32, sy as f32) as f64;
                gyw[i] = ip::sample(&gy, sx as f32, sy as f32) as f64;
            }
        }
        let count = valid.iter().filter(|&&v| v).count();
        if count < 10 {
            return None;
        }
        let (mut tm, mut imn) = (0.0, 0.0);
        for i in 0..n {
            if valid[i] {
                tm += t.data[i] as f64;
                imn += iw[i];
            }
        }
        tm /= count as f64;
        imn /= count as f64;
        let mut hess = vec![vec![0f64; 6]; 6];
        let (mut ip_, mut tp) = ([0f64; 6], [0f64; 6]);
        let (mut corr, mut inorm, mut tnorm) = (0.0, 0.0, 0.0);
        let mut tz = vec![0f64; n];
        let mut iz = vec![0f64; n];
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if !valid[i] {
                    continue;
                }
                tz[i] = t.data[i] as f64 - tm;
                iz[i] = iw[i] - imn;
                let (fx, fy) = (x as f64, y as f64);
                let j = [gxw[i] * fx, gyw[i] * fx, gxw[i] * fy, gyw[i] * fy, gxw[i], gyw[i]];
                for a in 0..6 {
                    for b in a..6 {
                        hess[a][b] += j[a] * j[b];
                    }
                    ip_[a] += j[a] * iz[i];
                    tp[a] += j[a] * tz[i];
                }
                corr += tz[i] * iz[i];
                inorm += iz[i] * iz[i];
                tnorm += tz[i] * tz[i];
            }
        }
        for a in 0..6 {
            for b in 0..a {
                hess[a][b] = hess[b][a];
            }
        }
        let rho = corr / (inorm.sqrt() * tnorm.sqrt()).max(1e-12);
        let iph = ip::solve(hess.clone(), ip_.to_vec())?;
        let lambda_n = inorm - (0..6).map(|a| ip_[a] * iph[a]).sum::<f64>();
        let lambda_d = corr - (0..6).map(|a| tp[a] * iph[a]).sum::<f64>();
        if lambda_d <= 0.0 {
            return None; // diverged
        }
        let lambda = lambda_n / lambda_d;
        let mut ep = [0f64; 6];
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if !valid[i] {
                    continue;
                }
                let e = lambda * tz[i] - iz[i];
                let (fx, fy) = (x as f64, y as f64);
                let j = [gxw[i] * fx, gyw[i] * fx, gxw[i] * fy, gyw[i] * fy, gxw[i], gyw[i]];
                for a in 0..6 {
                    ep[a] += j[a] * e;
                }
            }
        }
        let dp = ip::solve(hess, ep.to_vec())?;
        warp[0][0] += dp[0];
        warp[1][0] += dp[1];
        warp[0][1] += dp[2];
        warp[1][1] += dp[3];
        warp[0][2] += dp[4];
        warp[1][2] += dp[5];
        if (rho - last_rho).abs() < eps {
            break;
        }
        last_rho = rho;
    }
    Some(warp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecc_finds_a_shift() {
        let base = ImgF::from_fn(160, 120, |x, y| {
            let (x, y) = (x as f32, y as f32);
            128.0 + 60.0 * (x / 9.0).sin() * (y / 7.0).cos() + 30.0 * ((x + y) / 13.0).sin()
        });
        let shifted = ImgF::from_fn(160, 120, |x, y| ip::sample_replicate(&base, x as f32 - 3.0, y as f32 + 2.0));
        let w = find_transform_ecc(&base, &shifted, 100, 1e-6).unwrap();
        // The warp maps template pixels to input pixels: base(x, y) = shifted(x + 3, y - 2).
        assert!((w[0][2] - 3.0).abs() < 0.2 && (w[1][2] + 2.0).abs() < 0.2, "{w:?}");
    }
}
