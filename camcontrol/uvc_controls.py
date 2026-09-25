"""Direct access to the camera's controls through DirectShow (Windows only).

Why this exists:
  OpenCV's cap.set(CAP_PROP_EXPOSURE, ...) can't switch the camera's auto
  exposure off. If another program (HD2, for example) leaves the camera in
  auto exposure, OpenCV's exposure changes are silently ignored.
  The camera stores this mode itself, so it persists across programs.

  This module talks to the same Windows interfaces HD2 and the driver's
  settings dialog use (IAMCameraControl and IAMVideoProcAmp). It can read
  each control's range, current value and auto/manual flag, and set
  values in manual mode.

It opens its own handle on the device, separate from OpenCV's, so it can be
used while the camera is streaming.

Run directly to print every control the camera reports:
    python -m camcontrol.uvc_controls
"""

import ctypes
from ctypes import POINTER, byref, c_long, c_ulong, c_void_p
from ctypes.wintypes import DWORD, ULONG

import comtypes
from comtypes import COMMETHOD, GUID, HRESULT, IUnknown
from comtypes.automation import VARIANT
from comtypes.persist import IPropertyBag

# --- COM interface definitions (from the Windows SDK headers) -----------------

CLSID_SystemDeviceEnum = GUID("{62BE5D10-60EB-11d0-BD3B-00A0C911CE86}")
CLSID_VideoInputDeviceCategory = GUID("{860BB310-5D01-11d0-BD3B-00A0C911CE86}")
IID_IBaseFilter = GUID("{56A86895-0AD4-11CE-B03A-0020AF0BA770}")


class IEnumMoniker(IUnknown):
    _iid_ = GUID("{00000102-0000-0000-C000-000000000046}")


class IMoniker(IUnknown):
    _iid_ = GUID("{0000000F-0000-0000-C000-000000000046}")
    # Methods must be listed in vtable order. The ones we don't call are
    # given loose pointer types just to hold their place.
    _methods_ = [
        COMMETHOD([], HRESULT, "GetClassID", (["out"], POINTER(GUID))),   # IPersist
        COMMETHOD([], HRESULT, "IsDirty"),                                # IPersistStream
        COMMETHOD([], HRESULT, "Load", (["in"], c_void_p)),
        COMMETHOD([], HRESULT, "Save", (["in"], c_void_p), (["in"], ctypes.c_int)),
        COMMETHOD([], HRESULT, "GetSizeMax", (["out"], POINTER(ctypes.c_ulonglong))),
        COMMETHOD([], HRESULT, "BindToObject",                            # IMoniker
                  (["in"], c_void_p), (["in"], c_void_p),
                  (["in"], POINTER(GUID)), (["out"], POINTER(POINTER(IUnknown)))),
        COMMETHOD([], HRESULT, "BindToStorage",
                  (["in"], c_void_p), (["in"], c_void_p),
                  (["in"], POINTER(GUID)), (["out"], POINTER(POINTER(IUnknown)))),
    ]


IEnumMoniker._methods_ = [
    COMMETHOD([], HRESULT, "Next",
              (["in"], ULONG), (["out"], POINTER(POINTER(IMoniker))), (["out"], POINTER(ULONG))),
    COMMETHOD([], HRESULT, "Skip", (["in"], ULONG)),
    COMMETHOD([], HRESULT, "Reset"),
    COMMETHOD([], HRESULT, "Clone", (["out"], POINTER(POINTER(IEnumMoniker)))),
]


class ICreateDevEnum(IUnknown):
    _iid_ = GUID("{29840822-5B84-11D0-BD3B-00A0C911CE86}")
    _methods_ = [
        COMMETHOD([], HRESULT, "CreateClassEnumerator",
                  (["in"], POINTER(GUID)), (["out"], POINTER(POINTER(IEnumMoniker))), (["in"], DWORD)),
    ]


def _control_methods():
    """GetRange / Set / Get: identical layout for both control interfaces."""
    return [
        COMMETHOD([], HRESULT, "GetRange", (["in"], c_long),
                  (["out"], POINTER(c_long)), (["out"], POINTER(c_long)), (["out"], POINTER(c_long)),
                  (["out"], POINTER(c_long)), (["out"], POINTER(c_long))),
        COMMETHOD([], HRESULT, "Set", (["in"], c_long), (["in"], c_long), (["in"], c_long)),
        COMMETHOD([], HRESULT, "Get", (["in"], c_long),
                  (["out"], POINTER(c_long)), (["out"], POINTER(c_long))),
    ]


class IAMCameraControl(IUnknown):
    _iid_ = GUID("{C6E13370-30AC-11d0-A18C-00A0C9118956}")
    _methods_ = _control_methods()


class IAMVideoProcAmp(IUnknown):
    _iid_ = GUID("{C6E13360-30AC-11d0-A18C-00A0C9118956}")
    _methods_ = _control_methods()


FLAG_AUTO = 0x1
FLAG_MANUAL = 0x2

# Property ids from strmif.h. "camera" = IAMCameraControl, "video" = IAMVideoProcAmp.
CONTROLS = {
    "pan": ("camera", 0),
    "tilt": ("camera", 1),
    "roll": ("camera", 2),
    "zoom": ("camera", 3),
    "exposure": ("camera", 4),
    "iris": ("camera", 5),
    "focus": ("camera", 6),
    "brightness": ("video", 0),
    "contrast": ("video", 1),
    "hue": ("video", 2),
    "saturation": ("video", 3),
    "sharpness": ("video", 4),
    "gamma": ("video", 5),
    "white_balance": ("video", 6),
    "backlight_compensation": ("video", 8),
    "gain": ("video", 9),
}


# --- device lookup ---------------------------------------------------------------

def _ensure_com():
    # Each thread that uses COM must initialise it. Calling it twice is harmless.
    try:
        comtypes.CoInitializeEx(comtypes.COINIT_APARTMENTTHREADED)
    except OSError:
        pass  # already initialised in a different mode on this thread


def list_video_devices() -> list[tuple[str, "IMoniker"]]:
    """All DirectShow video input devices, in the same order as OpenCV's
    DirectShow camera indices."""
    _ensure_com()
    dev_enum = comtypes.CoCreateInstance(CLSID_SystemDeviceEnum, interface=ICreateDevEnum)
    try:
        enum = dev_enum.CreateClassEnumerator(byref(CLSID_VideoInputDeviceCategory), 0)
    except comtypes.COMError:
        return []
    if not enum:  # S_FALSE: no devices in this category
        return []
    devices = []
    while True:
        moniker, fetched = enum.Next(1)
        if not fetched:
            break
        bag = moniker.BindToStorage(None, None, byref(IPropertyBag._iid_)).QueryInterface(IPropertyBag)
        name = bag.Read("FriendlyName", VARIANT(), None)
        devices.append((str(name), moniker))
    return devices


class CameraControls:
    """Read and set one camera's controls.

        ctl = CameraControls(0)            # same index as OpenCV DirectShow
        ctl.get("exposure")                -> (value, is_auto)
        ctl.set("exposure", -9)            # also switches to manual
        ctl.set_auto("exposure", True)     # back to auto exposure
    """

    def __init__(self, index: int = 0):
        devices = list_video_devices()
        if index >= len(devices):
            raise RuntimeError(f"No DirectShow video device {index} ({len(devices)} found)")
        self.name, moniker = devices[index]
        base = moniker.BindToObject(None, None, byref(IID_IBaseFilter))
        self._ifaces = {
            "camera": self._query(base, IAMCameraControl),
            "video": self._query(base, IAMVideoProcAmp),
        }

    @staticmethod
    def _query(obj, iface):
        try:
            return obj.QueryInterface(iface)
        except comtypes.COMError:
            return None

    def _lookup(self, name):
        kind, prop = CONTROLS[name]
        iface = self._ifaces[kind]
        if iface is None:
            raise RuntimeError(f"Camera doesn't support {kind} controls")
        return iface, prop

    def range(self, name) -> dict | None:
        """{'min', 'max', 'step', 'default', 'can_auto', 'can_manual'}, or None if unsupported."""
        iface, prop = self._lookup(name)
        try:
            lo, hi, step, default, caps = iface.GetRange(prop)
        except comtypes.COMError:
            return None
        return {
            "min": lo, "max": hi, "step": step, "default": default,
            "can_auto": bool(caps & FLAG_AUTO), "can_manual": bool(caps & FLAG_MANUAL),
        }

    def get(self, name) -> tuple[int, bool]:
        """(value, is_auto)."""
        iface, prop = self._lookup(name)
        value, flags = iface.Get(prop)
        return value, bool(flags & FLAG_AUTO)

    def set(self, name, value: int, flags: int = FLAG_MANUAL):
        """Set a value, by default in manual mode (turns auto off for that control)."""
        iface, prop = self._lookup(name)
        iface.Set(prop, int(value), flags)

    def set_auto(self, name, on: bool):
        """Switch a control between auto and manual, keeping its current value."""
        iface, prop = self._lookup(name)
        value, _ = iface.Get(prop)
        iface.Set(prop, value, FLAG_AUTO if on else FLAG_MANUAL)


if __name__ == "__main__":
    for i, (name, _) in enumerate(list_video_devices()):
        print(f"[{i}] {name}")
    ctl = CameraControls(0)
    print(f"\nControls for [0] {ctl.name}:")
    for name in CONTROLS:
        r = ctl.range(name)
        if r is None:
            continue
        value, auto = ctl.get(name)
        modes = "/".join(m for m, ok in (("auto", r["can_auto"]), ("manual", r["can_manual"])) if ok)
        print(f"  {name:24} value {value:6}  {'AUTO' if auto else 'manual':6}  "
              f"range {r['min']}..{r['max']} step {r['step']} default {r['default']}  modes {modes}")
