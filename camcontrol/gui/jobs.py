"""Run slow work (stitching, stacking) off the GUI thread, with a busy dialog."""

from PySide6.QtCore import QThread, Signal
from PySide6.QtWidgets import QProgressDialog, QWidget


class Job(QThread):
    succeeded = Signal(object)  # whatever fn returned
    failed = Signal(str)

    def __init__(self, fn, parent=None):
        super().__init__(parent)
        self._fn = fn

    def run(self):
        try:
            result = self._fn()
        except Exception as e:  # show the problem instead of crashing
            self.failed.emit(str(e) or type(e).__name__)
            return
        self.succeeded.emit(result)


def run_job(parent: QWidget, message: str, fn, on_success, on_failure) -> Job:
    """Call fn() in a background thread while a "working" dialog is shown.

    on_success(result) or on_failure(message) is then called on the GUI thread.
    Keep a reference to the returned Job until it finishes.
    """
    busy = QProgressDialog(message, None, 0, 0, parent)  # no cancel button, endless bar
    busy.setWindowTitle("CamControl")
    busy.setMinimumDuration(0)
    busy.setModal(True)
    busy.show()

    job = Job(fn, parent)
    # Close the dialog first, so it isn't left over any message on_* shows.
    job.succeeded.connect(busy.close)
    job.failed.connect(busy.close)
    job.succeeded.connect(on_success)
    job.failed.connect(on_failure)
    job.finished.connect(job.deleteLater)
    job.start()
    return job
