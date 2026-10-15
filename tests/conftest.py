"""Shared test setup: an offscreen Qt app, fake cameras, and a main window
that only ever writes to a temporary folder (settings, captures, flats).

Run the tests from the project folder:
    venv\\Scripts\\python -m pytest            everything
    venv\\Scripts\\python -m pytest -m "not slow"   skip the long accuracy runs
"""

import os
import sys
import time
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")  # no windows pop up
os.environ.setdefault("QT_QPA_FONTDIR", "C:/Windows/Fonts")

import pytest
from PySide6.QtCore import QSettings
from PySide6.QtWidgets import QApplication, QFileDialog, QMessageBox

sys.path.insert(0, str(Path(__file__).parent))
import fakes  # noqa: E402  (needs the environment above)

ROOT = Path(__file__).resolve().parents[1]


@pytest.fixture(scope="session")
def qapp():
    return QApplication.instance() or QApplication(sys.argv)


def run_for(qapp, seconds: float):
    """Let the event loop run (frames arrive, background jobs finish)."""
    end = time.perf_counter() + seconds
    while time.perf_counter() < end:
        qapp.processEvents()
        time.sleep(0.002)


def run_until(qapp, condition, timeout: float = 10.0) -> bool:
    end = time.perf_counter() + timeout
    while time.perf_counter() < end:
        qapp.processEvents()
        if condition():
            return True
        time.sleep(0.005)
    return False


@pytest.fixture
def no_dialogs(monkeypatch):
    """Message boxes answer Yes / are recorded instead of blocking."""
    shown = []
    monkeypatch.setattr(QMessageBox, "question", lambda *a, **k: QMessageBox.StandardButton.Yes)
    monkeypatch.setattr(QMessageBox, "warning", lambda *a, **k: shown.append(("warning", a[2] if len(a) > 2 else a)))
    monkeypatch.setattr(QMessageBox, "information", lambda *a, **k: shown.append(("info", a[2] if len(a) > 2 else a)))
    monkeypatch.setattr(QFileDialog, "getSaveFileName", lambda *a, **k: ("", ""))
    monkeypatch.setattr(QFileDialog, "getOpenFileName", lambda *a, **k: ("", ""))
    return shown


@pytest.fixture
def window_factory(qapp, tmp_path, monkeypatch, no_dialogs):
    """make() -> a MainWindow with fake cameras ("FakeCam A", "FakeCam B"),
    settings in tmp_path/settings.ini, captures in tmp_path/captures. All
    windows made are closed (and their cameras stopped) after the test."""
    from camcontrol.gui import camera_link
    from camcontrol.gui import main_window as mw

    monkeypatch.setattr(mw, "QSettings", lambda *a: QSettings(str(tmp_path / "settings.ini"), QSettings.Format.IniFormat))
    monkeypatch.setattr(mw, "CAPTURE_DIR", tmp_path / "captures")
    monkeypatch.setattr(mw, "FLAT_DIR", tmp_path / "flats")
    monkeypatch.setattr(mw, "camera_list", lambda: list(fakes.DEVICES))
    monkeypatch.setattr(camera_link, "CameraWorker", fakes.FakeWorker)
    fakes.WORKERS.clear()
    made = []

    def make(save_on_close: bool = False):
        w = mw.MainWindow()
        if not save_on_close:
            w._save_settings = lambda: None
        w.resize(1500, 900)
        w.show()
        run_until(qapp, lambda: all(p.camera is None or p.camera.open or p.camera.status[0] != "Connecting..."
                                    for p in w.panes), 5)
        # Never the real captures folder.
        assert Path(w.folder_edit.text()).resolve().is_relative_to(tmp_path.resolve())
        made.append(w)
        return w

    yield make
    for w in made:
        w.close()
    for wk in fakes.WORKERS:
        wk.stop()


@pytest.fixture
def window(window_factory):
    return window_factory()
