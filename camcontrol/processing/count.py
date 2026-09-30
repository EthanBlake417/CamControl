"""Class counting: click on objects to mark them as one of up to 5 classes.

Only the data lives here (marks and totals, export); the Counting panel and
the image view handle the clicking and drawing.

Run directly for a self-test:
    python -m camcontrol.processing.count
"""

from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path

import pandas as pd

MAX_CLASSES = 5
# Distinct colors that show up on most images, as (R, G, B).
CLASS_COLORS = [(255, 60, 60), (60, 200, 255), (255, 220, 0), (200, 80, 255), (80, 255, 120)]


@dataclass
class Mark:
    x: float  # image pixels
    y: float
    cls: int  # index into the class list


@dataclass
class Counter:
    names: list[str] = field(default_factory=lambda: [f"Class {i + 1}" for i in range(MAX_CLASSES)])
    marks: list[Mark] = field(default_factory=list)

    def add(self, x: float, y: float, cls: int):
        self.marks.append(Mark(x, y, cls))

    def remove_nearest(self, x: float, y: float, max_distance: float) -> bool:
        """Remove the mark closest to (x, y) if it's within max_distance."""
        if not self.marks:
            return False
        d2 = [(m.x - x) ** 2 + (m.y - y) ** 2 for m in self.marks]
        i = min(range(len(d2)), key=d2.__getitem__)
        if d2[i] > max_distance ** 2:
            return False
        del self.marks[i]
        return True

    def counts(self) -> list[int]:
        totals = [0] * MAX_CLASSES
        for m in self.marks:
            totals[m.cls] += 1
        return totals

    def summary_rows(self) -> list[dict]:
        total = len(self.marks)
        return [{"Class": name, "Count": n, "Percent": round(100 * n / total, 1) if total else 0.0}
                for name, n in zip(self.names, self.counts())] + [
                   {"Class": "Total", "Count": total, "Percent": 100.0 if total else 0.0}]

    def mark_rows(self) -> list[dict]:
        return [{"#": i + 1, "Class": self.names[m.cls], "x (px)": round(m.x, 1), "y (px)": round(m.y, 1)}
                for i, m in enumerate(self.marks)]

    def export(self, path: Path, source: str):
        """.xlsx: Summary, Marks and Info sheets. .csv: the summary, plus
        the marks in a second file ending _marks.csv."""
        path = Path(path)
        summary = pd.DataFrame(self.summary_rows())
        marks = pd.DataFrame(self.mark_rows(), columns=["#", "Class", "x (px)", "y (px)"])
        if path.suffix.lower() == ".xlsx":
            info = pd.DataFrame({"Setting": ["Exported", "Image"],
                                 "Value": [datetime.now().isoformat(timespec="seconds"), source]})
            with pd.ExcelWriter(path, engine="openpyxl") as writer:
                summary.to_excel(writer, sheet_name="Summary", index=False)
                marks.to_excel(writer, sheet_name="Marks", index=False)
                info.to_excel(writer, sheet_name="Info", index=False)
        else:
            summary.to_csv(path, index=False, encoding="utf-8-sig")
            marks.to_csv(path.with_name(f"{path.stem}_marks.csv"), index=False, encoding="utf-8-sig")


if __name__ == "__main__":
    c = Counter()
    c.add(10, 10, 0); c.add(50, 50, 0); c.add(90, 20, 2)
    assert c.counts()[:3] == [2, 0, 1]
    assert not c.remove_nearest(200, 200, 5)
    assert c.remove_nearest(52, 49, 5) and c.counts()[0] == 1
    rows = c.summary_rows()
    print(rows[0], rows[-1])
    assert rows[-1]["Count"] == 2 and rows[0]["Percent"] == 50.0
    print("count OK")
