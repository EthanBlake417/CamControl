"""Export measurements to CSV or Excel.

One row per measurement. Columns that don't apply to a shape (e.g. area of
a line) are left empty. Units are in the column headers.
"""

from datetime import datetime
from pathlib import Path

import pandas as pd

from camcontrol.calibration import Calibration
from camcontrol.measure import KINDS, Measurement, compute

# (result key, column label, unit power: 0 = no unit, 1 = length, 2 = area)
FIELDS = [
    ("length", "Length", 1),
    ("perimeter", "Perimeter", 1),
    ("area", "Area", 2),
    ("diameter", "Diameter", 1),
    ("radius", "Radius", 1),
    ("width", "Width", 1),
    ("height", "Height", 1),
    ("angle_deg", "Angle (deg)", 0),
]


def column_name(label: str, power: int, unit: str) -> str:
    if power == 0:
        return label
    return f"{label} ({unit}{'²' if power == 2 else ''})"


def measurement_rows(measurements: list[Measurement], cal: Calibration, source: str) -> list[dict]:
    rows = []
    for m in measurements:
        results = compute(m, cal)
        row = {"#": m.id, "Type": KINDS[m.kind].label}
        for key, label, power in FIELDS:
            row[column_name(label, power, cal.unit)] = results.get(key)
        center = results.get("center_px")
        row["Centre x (px)"] = center[0] if center else None
        row["Centre y (px)"] = center[1] if center else None
        row["Points (px)"] = "; ".join(f"{x:.2f},{y:.2f}" for x, y in m.points)
        row["Image"] = source
        row["Calibration"] = cal.name
        rows.append(row)
    return rows


def export_table(rows: list[dict], path: Path, cal: Calibration, source: str):
    """Write rows to .csv or .xlsx (chosen by the file extension).

    Excel files get a second sheet, "Info", with the calibration and source.
    """
    df = pd.DataFrame(rows)
    path = Path(path)
    if path.suffix.lower() == ".xlsx":
        info = pd.DataFrame({
            "Setting": ["Exported", "Image", "Calibration", "Unit",
                        "X per pixel", "Y per pixel"],
            "Value": [datetime.now().isoformat(timespec="seconds"), source, cal.name,
                      cal.unit, cal.x, cal.y],
        })
        with pd.ExcelWriter(path, engine="openpyxl") as writer:
            df.to_excel(writer, sheet_name="Measurements", index=False)
            info.to_excel(writer, sheet_name="Info", index=False)
    else:
        # utf-8-sig so Excel shows µ and ² correctly when opening the CSV.
        df.to_csv(path, index=False, encoding="utf-8-sig")
