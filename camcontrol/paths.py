"""Locations of the project's asset files."""

from pathlib import Path

ASSETS = Path(__file__).resolve().parent.parent / "assets"
APP_ICON = ASSETS / "get_logo.ico"  # multi-size icon: window, taskbar
LOGO = ASSETS / "get_logo.png"      # larger logo: About box
