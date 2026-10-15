//! Flat-field correction: even out uneven lighting and remove fixed dust spots.
//!
//! A "flat" reference is an image of an empty, evenly lit field taken with the
//! same lighting and zoom as the samples. Correction divides it out:
//!     corrected = image * mean(flat) / flat        (per color channel)

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use image::RgbImage;

use crate::imgproc::{self as ip, ImgF};

pub struct FlatField {
    pub name: String,
    gain: Arc<[ImgF; 3]>,
    resized: Mutex<HashMap<(usize, usize), Arc<[ImgF; 3]>>>,
}

impl FlatField {
    pub fn new(flat: &RgbImage, name: &str) -> Self {
        let ch = ip::split(flat);
        let gain: Vec<ImgF> = ch
            .iter()
            .map(|c| {
                // A light blur takes out pixel noise; dust shadows are much wider and stay.
                let blurred = ip::gaussian_blur(c, 2.0).map(|v| v.max(1.0));
                let mean = blurred.data.iter().map(|&v| v as f64).sum::<f64>() / blurred.data.len() as f64;
                blurred.map(|v| (mean / v as f64) as f32)
            })
            .collect();
        let gain: [ImgF; 3] = gain.try_into().unwrap();
        Self { name: name.into(), gain: Arc::new(gain), resized: Mutex::new(HashMap::new()) }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.gain[0].w, self.gain[0].h)
    }

    /// The gain map at the image's size (a flat from 1920x1080 frames also
    /// works on 3264x1836 files, the same frames scaled up).
    fn gain_for(&self, w: usize, h: usize) -> Arc<[ImgF; 3]> {
        if (w, h) == self.size() {
            return self.gain.clone();
        }
        let mut cache = self.resized.lock().unwrap();
        cache
            .entry((w, h))
            .or_insert_with(|| {
                Arc::new([
                    ip::resize_linear(&self.gain[0], w, h),
                    ip::resize_linear(&self.gain[1], w, h),
                    ip::resize_linear(&self.gain[2], w, h),
                ])
            })
            .clone()
    }

    pub fn apply(&self, image: &RgbImage) -> RgbImage {
        let (w, h) = (image.width() as usize, image.height() as usize);
        let gain = self.gain_for(w, h);
        let mut out = image.clone();
        for (i, p) in out.pixels_mut().enumerate() {
            for c in 0..3 {
                p.0[c] = ip::sat_u8(p.0[c] as f32 * gain[c].data[i]);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn flattens_vignetting() {
        let (w, h) = (300u32, 200u32);
        let vignette = |x: u32, y: u32| {
            let (u, v) = ((x as f32 - w as f32 / 2.0) / w as f32, (y as f32 - h as f32 / 2.0) / h as f32);
            1.0 - 0.5 * (u * u + v * v) * 2.0
        };
        let flat = RgbImage::from_fn(w, h, |x, y| { let v = (200.0 * vignette(x, y)) as u8; Rgb([v, v, v]) });
        let sample = RgbImage::from_fn(w, h, |x, y| { let v = (120.0 * vignette(x, y)) as u8; Rgb([v, v, v]) });
        let ff = FlatField::new(&flat, "flat");
        let fixed = ff.apply(&sample);
        let std = |img: &RgbImage, m: u32| {
            let v: Vec<f64> = (m..h - m).flat_map(|y| (m..w - m).map(move |x| (x, y))).map(|(x, y)| img.get_pixel(x, y).0[0] as f64).collect();
            let mean = v.iter().sum::<f64>() / v.len() as f64;
            (v.iter().map(|a| (a - mean).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
        };
        assert!(std(&fixed, 10) < std(&sample, 0) / 5.0);
        let big = ip::resize_rgb(&sample, 600, 400, ip::Interp::Linear);
        assert_eq!(ff.apply(&big).dimensions(), (600, 400));
    }
}
