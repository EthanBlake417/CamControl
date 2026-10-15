//! Focus stacking (extended depth of field): for each pixel, the value from
//! the image that is sharpest there (strength of the Laplacian, smoothed).

use image::{GrayImage, Luma, RgbImage};

use crate::imgproc::{self as ip, ImgF};
use crate::processing::common::{align_all, check_same_size};

fn sharpness(image: &RgbImage, sigma: f64) -> ImgF {
    let k = ip::gaussian_kernel_n(3, 0.0);
    let gray = ip::sep_filter(&ImgF::from_gray(&ip::to_gray(image)), &k, &k).to_gray(); // ignore single-pixel noise
    let lap = ip::laplacian3(&ImgF::from_gray(&gray)).map(f32::abs);
    ip::gaussian_blur(&lap, sigma)
}

/// Combine a focus series. Returns (result, depth map: which image each pixel came from).
pub fn focus_stack(images: &[RgbImage], align: bool) -> Result<(RgbImage, GrayImage), String> {
    if images.len() < 2 {
        return Err("Focus stacking needs at least 2 images.".into());
    }
    check_same_size(images)?;
    let images = if align { align_all(images) } else { images.to_vec() };
    let (w, h) = images[0].dimensions();
    let sigma = (w as f64 / 500.0).max(2.0);
    let scores: Vec<ImgF> = std::thread::scope(|s| {
        let handles: Vec<_> = images.iter().map(|im| s.spawn(move || sharpness(im, sigma))).collect();
        handles.into_iter().map(|t| t.join().unwrap()).collect()
    });
    let mut result = RgbImage::new(w, h);
    let mut depth = GrayImage::new(w, h);
    let step = 255.0 / (images.len() - 1) as f64;
    for i in 0..(w * h) as usize {
        let best = (0..scores.len()).fold(0, |b, k| if scores[k].data[i] > scores[b].data[i] { k } else { b });
        let (x, y) = (i as u32 % w, i as u32 / w);
        result.put_pixel(x, y, *images[best].get_pixel(x, y));
        depth.put_pixel(x, y, Luma([(best as f64 * step) as u8]));
    }
    Ok((result, depth))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn takes_the_sharp_half_of_each() {
        let mut seed = 1u32;
        let mut sharp = ImgF::new(300, 200);
        for v in sharp.data.iter_mut() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            *v = (seed >> 24) as f32;
        }
        let blurry = ip::gaussian_blur(&sharp, 4.0);
        let make = |left_sharp: bool| RgbImage::from_fn(300, 200, |x, y| {
            let v = if (x < 150) == left_sharp { sharp.at(x as usize, y as usize) } else { blurry.at(x as usize, y as usize) };
            let v = ip::sat_u8(v);
            Rgb([v, v, v])
        });
        let (result, depth) = focus_stack(&[make(true), make(false)], false).unwrap();
        let mut err = 0.0;
        for y in 0..200 {
            for x in 10..290 {
                err += (result.get_pixel(x, y).0[0] as f64 - ip::sat_u8(sharp.at(x as usize, y as usize)) as f64).abs();
            }
        }
        assert!(err / (200.0 * 280.0) < 10.0);
        assert!(depth.get_pixel(50, 100).0[0] < 50 && depth.get_pixel(250, 100).0[0] > 200);
    }
}
