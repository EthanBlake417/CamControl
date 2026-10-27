//! Tables for export: Excel (.xlsx, one sheet each) or CSV (UTF-8 with a BOM,
//! so Excel shows µ and ² correctly). Measurements: one row per measurement,
//! units in the column headers, columns that don't apply left empty.

use std::path::Path;

use crate::calibration::Calibration;
use crate::measure::{compute, kind, Measurement};

#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Empty,
    Num(f64),
    Text(String),
    Bool(bool),
}

impl Cell {
    pub fn opt(v: Option<f64>) -> Cell {
        v.map(Cell::Num).unwrap_or(Cell::Empty)
    }

    pub fn csv(&self) -> String {
        match self {
            Cell::Empty => String::new(),
            Cell::Num(v) => format_num(*v),
            Cell::Bool(b) => if *b { "True".into() } else { "False".into() },
            Cell::Text(t) => {
                if t.contains([',', '"', '\n']) { format!("\"{}\"", t.replace('"', "\"\"")) } else { t.clone() }
            }
        }
    }
}

/// Like pandas writes floats: integers keep ".0", others as Python's repr (shortest round-trip).
fn format_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 { format!("{v:.1}") } else { format!("{v}") }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sheet {
    pub name: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
}

impl Sheet {
    pub fn new(name: &str, headers: &[&str], rows: Vec<Vec<Cell>>) -> Self {
        Self { name: name.into(), headers: headers.iter().map(|h| h.to_string()).collect(), rows }
    }

    pub fn from_strings(name: &str, headers: Vec<String>, rows: Vec<Vec<Cell>>) -> Self {
        Self { name: name.into(), headers, rows }
    }

    pub fn to_csv(&self) -> String {
        let mut out = String::from("\u{feff}");
        out += &self.headers.iter().map(|h| Cell::Text(h.clone()).csv()).collect::<Vec<_>>().join(",");
        out += "\n";
        for r in &self.rows {
            out += &r.iter().map(Cell::csv).collect::<Vec<_>>().join(",");
            out += "\n";
        }
        out
    }
}

pub fn write_csv(sheet: &Sheet, path: &Path) -> Result<(), String> {
    std::fs::write(path, sheet.to_csv()).map_err(|e| e.to_string())
}

pub fn write_xlsx(sheets: &[Sheet], path: &Path) -> Result<(), String> {
    use rust_xlsxwriter::{Format, Workbook};
    let mut wb = Workbook::new();
    let bold = Format::new().set_bold();
    for s in sheets {
        let ws = wb.add_worksheet();
        ws.set_name(&s.name).map_err(|e| e.to_string())?;
        for (c, h) in s.headers.iter().enumerate() {
            ws.write_string_with_format(0, c as u16, h, &bold).map_err(|e| e.to_string())?;
        }
        for (r, row) in s.rows.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                let (r, c) = (r as u32 + 1, c as u16);
                let result = match cell {
                    Cell::Empty => continue,
                    Cell::Num(v) => ws.write_number(r, c, *v).map(|_| ()),
                    Cell::Text(t) => ws.write_string(r, c, t).map(|_| ()),
                    Cell::Bool(b) => ws.write_boolean(r, c, *b).map(|_| ()),
                };
                result.map_err(|e| e.to_string())?;
            }
        }
        ws.autofit();
    }
    wb.save(path).map_err(|e| e.to_string())
}

/// .xlsx: all the sheets. .csv: the first sheet; any others go to files named
/// <stem>_<sheet name>.csv beside it (as the counting export's _marks.csv).
pub fn write_tables(sheets: &[Sheet], path: &Path, csv_extra: bool) -> Result<(), String> {
    let is_xlsx = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("xlsx"));
    if is_xlsx {
        return write_xlsx(sheets, path);
    }
    write_csv(&sheets[0], path)?;
    if csv_extra {
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        for s in &sheets[1..] {
            if s.name == "Info" {
                continue;
            }
            write_csv(s, &path.with_file_name(format!("{stem}_{}.csv", s.name.to_lowercase())))?;
        }
    }
    Ok(())
}

pub fn info_sheet(entries: &[(&str, Cell)]) -> Sheet {
    let mut rows = vec![vec![Cell::Text("Exported".into()), Cell::Text(now_iso())]];
    rows.extend(entries.iter().map(|(k, v)| vec![Cell::Text(k.to_string()), v.clone()]));
    Sheet::new("Info", &["Setting", "Value"], rows)
}

pub fn now_iso() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

// --- measurements -------------------------------------------------------------------

/// (result key, column label, unit power: 0 = no unit, 1 = length, 2 = area)
const FIELDS: [(&str, &str, u8); 8] = [
    ("length", "Length", 1),
    ("perimeter", "Perimeter", 1),
    ("area", "Area", 2),
    ("diameter", "Diameter", 1),
    ("radius", "Radius", 1),
    ("width", "Width", 1),
    ("height", "Height", 1),
    ("angle_deg", "Angle (deg)", 0),
];

pub fn column_name(label: &str, power: u8, unit: &str) -> String {
    match power {
        0 => label.into(),
        2 => format!("{label} ({unit}²)"),
        _ => format!("{label} ({unit})"),
    }
}

pub fn measurement_sheet(measurements: &[Measurement], cal: &Calibration, source: &str) -> Sheet {
    let mut headers = vec!["#".to_string(), "Type".to_string()];
    headers.extend(FIELDS.iter().map(|(_, l, p)| column_name(l, *p, &cal.unit)));
    headers.extend(["Center x (px)", "Center y (px)", "Points (px)", "Image", "Calibration"].map(String::from));
    let rows = measurements
        .iter()
        .filter_map(|m| {
            let r = compute(m, cal).ok()?;
            let mut row = vec![Cell::Num(m.id as f64), Cell::Text(kind(&m.kind).label.into())];
            row.extend(FIELDS.iter().map(|(k, _, _)| Cell::opt(r.get(k))));
            row.push(Cell::opt(r.center_px.map(|c| c.0)));
            row.push(Cell::opt(r.center_px.map(|c| c.1)));
            row.push(Cell::Text(m.points.iter().map(|(x, y)| format!("{x:.2},{y:.2}")).collect::<Vec<_>>().join("; ")));
            row.push(Cell::Text(source.into()));
            row.push(Cell::Text(cal.name.clone()));
            Some(row)
        })
        .collect();
    Sheet::from_strings("Measurements", headers, rows)
}

pub fn export_measurements(measurements: &[Measurement], cal: &Calibration, source: &str, path: &Path) -> Result<(), String> {
    let sheet = measurement_sheet(measurements, cal, source);
    let info = info_sheet(&[
        ("Image", Cell::Text(source.into())),
        ("Calibration", Cell::Text(cal.name.clone())),
        ("Unit", Cell::Text(cal.unit.clone())),
        ("X per pixel", Cell::Num(cal.x)),
        ("Y per pixel", Cell::Num(cal.y)),
    ]);
    write_tables(&[sheet, info], path, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_and_xlsx() {
        let m = vec![Measurement { id: 1, kind: "line".into(), points: vec![(0.0, 0.0), (3.0, 4.0)] }];
        let s = measurement_sheet(&m, &Calibration::pixels(), "img.png");
        let csv = s.to_csv();
        assert!(csv.starts_with('\u{feff}'));
        assert!(csv.contains("Length (px)") && csv.contains(",5.0,"), "{csv}");
        let dir = std::env::temp_dir().join(format!("camcontrol-rs-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        export_measurements(&m, &Calibration::pixels(), "img.png", &dir.join("m.xlsx")).unwrap();
        assert!(std::fs::metadata(dir.join("m.xlsx")).unwrap().len() > 1000);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
