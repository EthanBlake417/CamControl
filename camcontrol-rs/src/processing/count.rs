//! Class counting: marks clicked on objects, as one of up to 5 classes.

use crate::export::{Cell, Sheet};

pub const MAX_CLASSES: usize = 5;
/// (R, G, B), distinct on most images.
pub const CLASS_COLORS: [[u8; 3]; MAX_CLASSES] = [[255, 60, 60], [60, 200, 255], [255, 220, 0], [200, 80, 255], [80, 255, 120]];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    pub x: f64,
    pub y: f64,
    pub cls: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Counter {
    pub marks: Vec<Mark>,
}

impl Default for Counter {
    fn default() -> Self {
        Self { marks: Vec::new() }
    }
}

pub fn default_names() -> Vec<String> {
    (1..=MAX_CLASSES).map(|i| format!("Class {i}")).collect()
}

impl Counter {
    pub fn add(&mut self, x: f64, y: f64, cls: usize) {
        self.marks.push(Mark { x, y, cls });
    }

    /// Remove the mark closest to (x, y) if it's within max_distance.
    pub fn remove_nearest(&mut self, x: f64, y: f64, max_distance: f64) -> bool {
        let Some((i, d2)) = self
            .marks
            .iter()
            .enumerate()
            .map(|(i, m)| (i, (m.x - x).powi(2) + (m.y - y).powi(2)))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        else {
            return false;
        };
        if d2 > max_distance * max_distance {
            return false;
        }
        self.marks.remove(i);
        true
    }

    pub fn counts(&self) -> [usize; MAX_CLASSES] {
        let mut totals = [0; MAX_CLASSES];
        for m in &self.marks {
            totals[m.cls] += 1;
        }
        totals
    }

    pub fn summary_sheet(&self, names: &[String]) -> Sheet {
        let total = self.marks.len();
        let pct = |n: usize| if total > 0 { (1000.0 * n as f64 / total as f64).round() / 10.0 } else { 0.0 };
        let mut rows: Vec<Vec<Cell>> = names
            .iter()
            .zip(self.counts())
            .map(|(name, n)| vec![Cell::Text(name.clone()), Cell::Num(n as f64), Cell::Num(pct(n))])
            .collect();
        rows.push(vec![Cell::Text("Total".into()), Cell::Num(total as f64), Cell::Num(if total > 0 { 100.0 } else { 0.0 })]);
        Sheet::new("Summary", &["Class", "Count", "Percent"], rows)
    }

    pub fn marks_sheet(&self, names: &[String]) -> Sheet {
        let rows = self
            .marks
            .iter()
            .enumerate()
            .map(|(i, m)| vec![
                Cell::Num((i + 1) as f64),
                Cell::Text(names[m.cls].clone()),
                Cell::Num((m.x * 10.0).round() / 10.0),
                Cell::Num((m.y * 10.0).round() / 10.0),
            ])
            .collect();
        Sheet::new("Marks", &["#", "Class", "x (px)", "y (px)"], rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_removes() {
        let mut c = Counter::default();
        c.add(10.0, 10.0, 0);
        c.add(50.0, 50.0, 0);
        c.add(90.0, 20.0, 2);
        assert_eq!(&c.counts()[..3], &[2, 0, 1]);
        assert!(!c.remove_nearest(200.0, 200.0, 5.0));
        assert!(c.remove_nearest(52.0, 49.0, 5.0) && c.counts()[0] == 1);
        let s = c.summary_sheet(&default_names());
        assert_eq!(s.rows[0][2], Cell::Num(50.0));
        assert_eq!(s.rows[5][1], Cell::Num(2.0));
    }
}
