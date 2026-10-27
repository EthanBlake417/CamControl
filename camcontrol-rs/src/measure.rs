//! Measurement geometry (no GUI code). A Measurement stores only what the user
//! clicked: its kind and points in image pixels. Results are computed for a
//! given Calibration: points are converted to physical units per axis first,
//! so non-square pixels come out right.

use std::f64::consts::PI;

use crate::calibration::Calibration;

pub type Point = (f64, f64);

pub struct Kind {
    pub name: &'static str,
    pub label: &'static str,
    /// Fixed number of clicks, or None = keep clicking until finished.
    pub n_points: Option<usize>,
    pub min_points: usize,
}

pub const KINDS: [Kind; 7] = [
    Kind { name: "line", label: "Line", n_points: Some(2), min_points: 2 },
    Kind { name: "polyline", label: "Polyline", n_points: None, min_points: 2 },
    Kind { name: "circle", label: "Circle (3 pt)", n_points: Some(3), min_points: 3 },
    Kind { name: "circle_centre", label: "Circle (center)", n_points: Some(2), min_points: 2 },
    Kind { name: "angle", label: "Angle", n_points: Some(3), min_points: 3 },
    Kind { name: "rectangle", label: "Rectangle", n_points: Some(2), min_points: 2 },
    Kind { name: "polygon", label: "Polygon", n_points: None, min_points: 3 },
];

pub fn kind(name: &str) -> &'static Kind {
    KINDS.iter().find(|k| k.name == name).unwrap_or(&KINDS[0])
}

pub fn is_circle(kind: &str) -> bool {
    kind == "circle" || kind == "circle_centre"
}

#[derive(Clone, Debug, PartialEq)]
pub struct Measurement {
    pub id: usize,
    pub kind: String,
    pub points: Vec<Point>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Results {
    pub length: Option<f64>,
    pub perimeter: Option<f64>,
    pub area: Option<f64>,
    pub radius: Option<f64>,
    pub diameter: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub angle_deg: Option<f64>,
    /// Circle center in image pixels, for drawing.
    pub center_px: Option<Point>,
}

impl Results {
    pub fn get(&self, key: &str) -> Option<f64> {
        match key {
            "length" => self.length,
            "perimeter" => self.perimeter,
            "area" => self.area,
            "radius" => self.radius,
            "diameter" => self.diameter,
            "width" => self.width,
            "height" => self.height,
            "angle_deg" => self.angle_deg,
            _ => None,
        }
    }
}

pub fn distance(a: Point, b: Point) -> f64 {
    (b.0 - a.0).hypot(b.1 - a.1)
}

pub fn path_length(p: &[Point], closed: bool) -> f64 {
    let mut total: f64 = p.windows(2).map(|w| distance(w[0], w[1])).sum();
    if closed && p.len() > 2 {
        total += distance(p[p.len() - 1], p[0]);
    }
    total
}

pub fn polygon_area(p: &[Point]) -> f64 {
    let n = p.len();
    (0..n).map(|i| p[i].0 * p[(i + 1) % n].1 - p[(i + 1) % n].0 * p[i].1).sum::<f64>().abs() / 2.0
}

/// Center and radius of the circle through three points.
pub fn circle_through(a: Point, b: Point, c: Point) -> Result<(f64, f64, f64), String> {
    let ((ax, ay), (bx, by), (cx, cy)) = (a, b, c);
    let d = 2.0 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));
    let longest = distance(a, b).max(distance(b, c)).max(distance(c, a));
    if longest == 0.0 || d.abs() < 1e-6 * longest * longest {
        return Err("the three points are (nearly) in a straight line".into());
    }
    let (a2, b2, c2) = (ax * ax + ay * ay, bx * bx + by * by, cx * cx + cy * cy);
    let ux = (a2 * (by - cy) + b2 * (cy - ay) + c2 * (ay - by)) / d;
    let uy = (a2 * (cx - bx) + b2 * (ax - cx) + c2 * (bx - ax)) / d;
    Ok((ux, uy, distance((ux, uy), a)))
}

/// Angle a-vertex-b in degrees (0 to 180).
pub fn angle_at(a: Point, v: Point, b: Point) -> Result<f64, String> {
    let (v1, v2) = ((a.0 - v.0, a.1 - v.1), (b.0 - v.0, b.1 - v.1));
    if v1 == (0.0, 0.0) || v2 == (0.0, 0.0) {
        return Err("an angle arm has zero length".into());
    }
    let cross = v1.0 * v2.1 - v1.1 * v2.0;
    let dot = v1.0 * v2.0 + v1.1 * v2.1;
    Ok(cross.abs().atan2(dot).to_degrees())
}

/// Results for a measurement, in the calibration's units. Err if the points don't make a valid shape.
pub fn compute(m: &Measurement, cal: &Calibration) -> Result<Results, String> {
    let k = kind(&m.kind);
    if m.points.len() < k.min_points {
        return Err(format!("{} needs at least {} points", k.label, k.min_points));
    }
    let p: Vec<Point> = m.points.iter().map(|&(x, y)| (x * cal.x, y * cal.y)).collect();
    let mut r = Results::default();
    match k.name {
        "line" => r.length = Some(distance(p[0], p[1])),
        "polyline" => r.length = Some(path_length(&p, false)),
        "circle" | "circle_centre" => {
            let (cx, cy, rad) = if k.name == "circle" {
                circle_through(p[0], p[1], p[2])?
            } else {
                let rad = distance(p[0], p[1]);
                if rad == 0.0 {
                    return Err("the circle has zero radius".into());
                }
                (p[0].0, p[0].1, rad)
            };
            r.radius = Some(rad);
            r.diameter = Some(2.0 * rad);
            r.perimeter = Some(2.0 * PI * rad);
            r.area = Some(PI * rad * rad);
            r.center_px = Some((cx / cal.x, cy / cal.y));
        }
        "angle" => r.angle_deg = Some(angle_at(p[0], p[1], p[2])?),
        "rectangle" => {
            let (w, h) = ((p[1].0 - p[0].0).abs(), (p[1].1 - p[0].1).abs());
            r.width = Some(w);
            r.height = Some(h);
            r.area = Some(w * h);
            r.perimeter = Some(2.0 * (w + h));
        }
        "polygon" => {
            r.area = Some(polygon_area(&p));
            r.perimeter = Some(path_length(&p, true));
        }
        other => return Err(format!("unknown measurement kind {other:?}")),
    }
    Ok(r)
}

pub fn fmt(v: f64) -> String {
    format!("{v:.2}")
}

/// Short text for the on-image label, e.g. "123.45 px" or "90.00°".
pub fn summary(m: &Measurement, r: &Results, cal: &Calibration) -> String {
    let u = &cal.unit;
    match m.kind.as_str() {
        "line" | "polyline" => format!("{} {u}", fmt(r.length.unwrap_or(0.0))),
        "circle" | "circle_centre" => format!("⌀ {} {u}", fmt(r.diameter.unwrap_or(0.0))),
        "angle" => format!("{}°", fmt(r.angle_deg.unwrap_or(0.0))),
        "rectangle" => format!("{} × {} {u}", fmt(r.width.unwrap_or(0.0)), fmt(r.height.unwrap_or(0.0))),
        "polygon" => format!("A {} {u}²", fmt(r.area.unwrap_or(0.0))),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(kind: &str, points: &[Point]) -> Measurement {
        Measurement { id: 1, kind: kind.into(), points: points.to_vec() }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn self_test_cases() {
        let px = Calibration::pixels();
        assert!(close(compute(&m("line", &[(0.0, 0.0), (3.0, 4.0)]), &px).unwrap().length.unwrap(), 5.0));
        assert!(close(compute(&m("polyline", &[(0.0, 0.0), (3.0, 4.0), (3.0, 10.0)]), &px).unwrap().length.unwrap(), 11.0));
        let c = compute(&m("circle", &[(0.0, 10.0), (10.0, 0.0), (-10.0, 0.0)]), &px).unwrap();
        assert!(close(c.radius.unwrap(), 10.0) && close(c.center_px.unwrap().0, 0.0));
        let c = compute(&m("circle_centre", &[(5.0, 5.0), (8.0, 9.0)]), &px).unwrap();
        assert!(close(c.radius.unwrap(), 5.0) && close(c.center_px.unwrap().1, 5.0));
        assert!(close(compute(&m("angle", &[(10.0, 0.0), (0.0, 0.0), (0.0, 10.0)]), &px).unwrap().angle_deg.unwrap(), 90.0));
        assert!(close(compute(&m("angle", &[(10.0, 0.0), (0.0, 0.0), (10.0, 10.0)]), &px).unwrap().angle_deg.unwrap(), 45.0));
        let r = compute(&m("rectangle", &[(2.0, 3.0), (12.0, 8.0)]), &px).unwrap();
        assert!(close(r.area.unwrap(), 50.0) && close(r.perimeter.unwrap(), 30.0));
        let sq = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let r = compute(&m("polygon", &sq), &px).unwrap();
        assert!(close(r.area.unwrap(), 100.0) && close(r.perimeter.unwrap(), 40.0));
        // Non-square pixels: 2 µm/px in X, 1 µm/px in Y.
        let cal = Calibration { name: "test".into(), x: 2.0, y: 1.0, unit: "µm".into() };
        assert!(close(compute(&m("line", &[(0.0, 0.0), (10.0, 0.0)]), &cal).unwrap().length.unwrap(), 20.0));
        assert!(close(compute(&m("line", &[(0.0, 0.0), (0.0, 10.0)]), &cal).unwrap().length.unwrap(), 10.0));
        assert!(close(compute(&m("circle", &[(5.0, 0.0), (0.0, 10.0), (-5.0, 0.0)]), &cal).unwrap().radius.unwrap(), 10.0));
        assert!(close(compute(&m("angle", &[(5.0, 0.0), (0.0, 0.0), (5.0, 10.0)]), &cal).unwrap().angle_deg.unwrap(), 45.0));
        assert!(compute(&m("circle", &[(0.0, 0.0), (5.0, 5.0), (10.0, 10.0)]), &px).is_err());
    }
}
