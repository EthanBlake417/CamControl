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

  It also lists the cameras (list_video_devices) and the frame sizes and
  formats each one offers (list_modes, through IAMStreamConfig), which
  OpenCV can't do.

It opens its own handle on the device, separate from OpenCV's, so it can be
used while the camera is streaming. Cameras differ in which controls they
have: range() returns None for any the camera doesn't support.

Run directly to print every camera, its sizes and the first one's controls:
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


class AM_MEDIA_TYPE(ctypes.Structure):
    _fields_ = [
        ("majortype", GUID),
        ("subtype", GUID),
        ("bFixedSizeSamples", ctypes.c_int),
        ("bTemporalCompression", ctypes.c_int),
        ("lSampleSize", ULONG),
        ("formattype", GUID),
        ("pUnk", c_void_p),
        ("cbFormat", ULONG),
        ("pbFormat", c_void_p),
    ]


class IAMStreamConfig(IUnknown):
    _iid_ = GUID("{C6E13340-30AC-11d0-A18C-00A0C9118956}")
    _methods_ = [
        COMMETHOD([], HRESULT, "SetFormat", (["in"], POINTER(AM_MEDIA_TYPE))),
        COMMETHOD([], HRESULT, "GetFormat", (["out"], POINTER(POINTER(AM_MEDIA_TYPE)))),
        COMMETHOD([], HRESULT, "GetNumberOfCapabilities",
                  (["out"], POINTER(ctypes.c_int)), (["out"], POINTER(ctypes.c_int))),
        COMMETHOD([], HRESULT, "GetStreamCaps",
                  (["in"], ctypes.c_int), (["out"], POINTER(POINTER(AM_MEDIA_TYPE))), (["in"], c_void_p)),
    ]


class IPin(IUnknown):
    _iid_ = GUID("{56A86891-0AD4-11CE-B03A-0020AF0BA770}")


class IEnumPins(IUnknown):
    _iid_ = GUID("{56A86892-0AD4-11CE-B03A-0020AF0BA770}")
    _methods_ = [
        COMMETHOD([], HRESULT, "Next",
                  (["in"], ULONG), (["out"], POINTER(POINTER(IPin))), (["out"], POINTER(ULONG))),
    ]


class IBaseFilter(IUnknown):
    _iid_ = IID_IBaseFilter
    _methods_ = [
        COMMETHOD([], HRESULT, "GetClassID", (["out"], POINTER(GUID))),   # IPersist
        COMMETHOD([], HRESULT, "Stop"),                                   # IMediaFilter
        COMMETHOD([], HRESULT, "Pause"),
        COMMETHOD([], HRESULT, "Run", (["in"], ctypes.c_longlong)),
        COMMETHOD([], HRESULT, "GetState", (["in"], DWORD), (["out"], POINTER(ctypes.c_int))),
        COMMETHOD([], HRESULT, "SetSyncSource", (["in"], c_void_p)),
        COMMETHOD([], HRESULT, "GetSyncSource", (["out"], POINTER(c_void_p))),
        COMMETHOD([], HRESULT, "EnumPins", (["out"], POINTER(POINTER(IEnumPins)))),  # IBaseFilter
    ]


FORMAT_VideoInfo = GUID("{05589F80-C356-11CE-BF01-00AA0055595A}")
FORMAT_VideoInfo2 = GUID("{F72A76A0-EB0A-11D0-ACE4-0000C0CC16BA}")
# Where the BITMAPINFOHEADER starts in each format block.
_BMI_OFFSET = {str(FORMAT_VideoInfo): 48, str(FORMAT_VideoInfo2): 72}
_AVG_TIME_OFFSET = 40  # AvgTimePerFrame (100 ns units), same place in both

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


def _fourcc(subtype: GUID) -> str:
    """'MJPG', 'YUY2', 'NV12', ... from a video subtype GUID (its first 4 bytes),
    or 'RGB' etc. for the few subtypes that aren't FOURCCs."""
    code = bytes(subtype)[:4]
    if all(32 <= b < 127 for b in code):
        return code.decode("ascii").strip()
    return "other"


def list_modes(index: int) -> list[dict]:
    """Frame sizes and formats camera `index` offers:
    [{'width', 'height', 'fourcc', 'fps'}, ...], fps being the fastest it
    allows at that size and format. Empty if the camera won't say."""
    devices = list_video_devices()
    if index >= len(devices):
        return []
    try:
        base = devices[index][1].BindToObject(None, None, byref(IID_IBaseFilter)).QueryInterface(IBaseFilter)
        pins = base.EnumPins()
        config = None
        # The first pin with formats to offer is the capture pin.
        while config is None:
            pin, fetched = pins.Next(1)
            if not fetched:
                return []
            try:
                config = pin.QueryInterface(IAMStreamConfig)
            except comtypes.COMError:
                pass
        count, size = config.GetNumberOfCapabilities()
        caps = ctypes.create_string_buffer(max(size, 128))
        modes = []
        for i in range(count):
            pmt = config.GetStreamCaps(i, ctypes.addressof(caps))
            mt = pmt.contents
            try:
                offset = _BMI_OFFSET.get(str(mt.formattype))
                if offset is None or not mt.pbFormat or mt.cbFormat < offset + 12:
                    continue
                frame_time = ctypes.c_longlong.from_address(mt.pbFormat + _AVG_TIME_OFFSET).value
                width = ctypes.c_long.from_address(mt.pbFormat + offset + 4).value
                height = abs(ctypes.c_long.from_address(mt.pbFormat + offset + 8).value)
                modes.append({
                    "width": width, "height": height, "fourcc": _fourcc(mt.subtype),
                    "fps": round(1e7 / frame_time, 1) if frame_time > 0 else None,
                })
            finally:
                if mt.pbFormat:
                    ctypes.windll.ole32.CoTaskMemFree(c_void_p(mt.pbFormat))
                ctypes.windll.ole32.CoTaskMemFree(pmt)
        return modes
    except comtypes.COMError:
        return []


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
        if self._ifaces[CONTROLS[name][0]] is None:
            return None  # the camera has none of this kind of control
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
        for m in list_modes(i):
            print(f"      {m['width']} x {m['height']}  {m['fourcc']:5} {m['fps']} fps")
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
