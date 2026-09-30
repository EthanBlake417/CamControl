"""Settings files: the app's setup saved as a JSON file you can name, open
later, keep per job (e.g. "brightfield 10x.json") or copy to another PC.

File > Open settings / Open recent settings / Save settings / Save settings as.
The most recently opened or saved file is opened again when the app starts.

A settings file holds (see MainWindow.current_settings):
    camera       slider values: exposure, gain, contrast, saturation, sharpness
    capture      frames averaged, save size, file format
    output       folder and file name for captures, videos and time-lapses
    flat_field   reference image path and whether correction is on
    recording    video and time-lapse settings
    view         grid and crosshair on/off

Run directly to print a settings file:
    python -m camcontrol.settings_file "settings/brightfield 10x.json"
"""

import json
import sys
from datetime import datetime
from pathlib import Path

SETTINGS_DIR = Path(__file__).resolve().parent.parent / "settings"  # default place to save them
MAX_RECENT = 8
FILE_FILTER = "CamControl settings (*.json);;All files (*)"


def save_settings_file(path, data: dict):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    data = {"camcontrol_settings": 1, "saved": datetime.now().isoformat(timespec="seconds"), **data}
    path.write_text(json.dumps(data, indent=2), encoding="utf-8")


def load_settings_file(path) -> dict:
    data = json.loads(Path(path).read_text(encoding="utf-8"))
    if not isinstance(data, dict) or "camcontrol_settings" not in data:
        raise ValueError("This isn't a CamControl settings file.")
    return data


def add_recent(recent: list[str], path) -> list[str]:
    """Put path first in the recent list (no duplicates, at most MAX_RECENT)."""
    path = str(Path(path).resolve())
    rest = [p for p in recent if Path(p).resolve() != Path(path)]
    return [path] + rest[: MAX_RECENT - 1]


if __name__ == "__main__":
    print(json.dumps(load_settings_file(sys.argv[1]), indent=2))
