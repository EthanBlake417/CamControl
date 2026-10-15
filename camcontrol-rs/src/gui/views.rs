//! Several views (views.py): layouts, and "Save all as one image".

use image::{Rgb, RgbImage};

use crate::draw::{Canvas, SkiaCanvas};
use crate::imgproc::{self as ip, Interp};

pub const MAX_VIEWS: usize = 12;
const GAP_PX: u32 = 8; // white gap between images in a saved grid
/// name, label
pub const LAYOUTS: [(&str, &str); 4] = [("single", "Single"), ("row", "Side by side"), ("column", "Stacked"), ("grid", "Grid")];

/// Columns that show n images of this aspect ratio largest in width x height.
pub fn best_columns(n: usize, width: f32, height: f32, aspect: f32) -> usize {
    let mut best = (1, -1.0f32);
    for cols in 1..=n.max(1) {
        let rows = n.div_ceil(cols);
        let size = (width / cols as f32).min(height / rows as f32 * aspect);
        if size > best.1 + 1e-6 {
            best = (cols, size);
        }
    }
    best.0
}

pub fn columns_for(layout: &str, n: usize, width: f32, height: f32) -> usize {
    match layout {
        "row" => n.max(1),
        "column" | "single" => 1,
        _ => best_columns(n, width, height, 16.0 / 9.0),
    }
}

/// The images in a grid (as square as possible), all at the smallest one's
/// height, each with its name in the top-left corner.
pub fn grid_image(images: &[&RgbImage], names: &[String]) -> RgbImage {
    let h = images.iter().map(|i| i.height()).min().unwrap_or(1);
    let tiles: Vec<RgbImage> = images
        .iter()
        .zip(names)
        .map(|(im, name)| {
            let im = if im.height() == h {
                (*im).clone()
            } else {
                let w = (im.width() as f64 * h as f64 / im.height() as f64).round() as u32;
                ip::resize_rgb(im, w.max(1), h, Interp::Area)
            };
            let scale = (h as f32 / 1000.0).max(0.6);
            let px = 22.0 * scale;
            let pad = 8.0 * scale;
            let mut c = SkiaCanvas::from_image(&im);
            let tw = c.text_width(name, px);
            c.fill_rect((0.0, 0.0), (tw + 2.0 * pad, px * 1.2 + 2.0 * pad), [0, 0, 0, 255]);
            c.text((pad, pad), name, px, [255, 255, 255, 255]);
            c.to_image()
        })
        .collect();
    let cols = (tiles.len() as f64).sqrt().ceil() as usize;
    let rows: Vec<&[RgbImage]> = tiles.chunks(cols.max(1)).collect();
    let row_w = |r: &[RgbImage]| r.iter().map(|t| t.width()).sum::<u32>() + GAP_PX * (r.len() as u32 - 1);
    let width = rows.iter().map(|r| row_w(r)).max().unwrap_or(1);
    let height = rows.len() as u32 * h + GAP_PX * (rows.len() as u32 - 1);
    let mut out = RgbImage::from_pixel(width, height, Rgb([255, 255, 255]));
    for (ri, r) in rows.iter().enumerate() {
        let mut x = 0;
        for t in r.iter() {
            image::imageops::replace(&mut out, t, x as i64, (ri as u32 * (h + GAP_PX)) as i64);
            x += t.width() + GAP_PX;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts() {
        assert_eq!(best_columns(4, 1600.0, 900.0, 16.0 / 9.0), 2);
        assert_eq!(best_columns(3, 3000.0, 400.0, 16.0 / 9.0), 3);
        let a = RgbImage::new(160, 90);
        let b = RgbImage::new(320, 180);
        let g = grid_image(&[&a, &b, &a], &["1".into(), "2".into(), "3".into()]);
        assert_eq!(g.height(), 90 * 2 + GAP_PX);
    }
}
