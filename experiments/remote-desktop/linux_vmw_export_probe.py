"""Bounded vmwgfx PRIME diagnosis; root, active GNOME Wayland only.

Compare an independent card fd with the compositor's duplicated card fd. GETFB2
creates NEW GEM handles (drm_gem_handle_create); close only those exact handles.
Never drop/acquire master, modeset, export with DRM_RDWR, or touch old handles.
Optional one-frame pixel reader; no input, framebuffer replacement, or settings.
Locked-session inspection requires --expect-locked; this tool never locks it.
This is an experiment, not a production capture backend.
"""
import ctypes as C
import json
import os
import pathlib
import platform
import subprocess
import argparse


class FB2(C.Structure):
    _fields_ = [(n, C.c_uint32) for n in ("fb_id", "width", "height", "pixel_format")]
    _fields_ += [("modifier", C.c_uint64), ("flags", C.c_uint32)]
    _fields_ += [(n, C.c_uint32 * 4) for n in ("handles", "pitches", "offsets")]


class GemClose(C.Structure):
    _fields_ = [("handle", C.c_uint32), ("pad", C.c_uint32)]


class CrtcPrefix(C.Structure):
    _fields_ = [("crtc_id", C.c_uint32), ("buffer_id", C.c_uint32)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pixel-reader", type=pathlib.Path)
    parser.add_argument("--output", type=pathlib.Path)
    parser.add_argument("--expect-locked", action="store_true")
    args = parser.parse_args()
    assert bool(args.pixel_reader) == bool(args.output)
    if args.pixel_reader:
        assert args.pixel_reader.is_absolute() and args.pixel_reader.is_file()
        assert args.output.is_absolute() and not args.output.exists()
        assert args.output.parent.stat().st_uid == 0
        assert args.output.parent.stat().st_mode & 0o077 == 0
    assert os.geteuid() == 0 and platform.machine() == "x86_64"
    assert pathlib.Path("/sys/class/drm/card0/device/driver").resolve().name == "vmwgfx"
    session = subprocess.check_output(
        ["loginctl", "show-seat", "seat0", "-p", "ActiveSession", "--value"], text=True).strip()
    def state():
        return dict(line.split("=", 1) for line in subprocess.check_output(
            ["loginctl", "show-session", session, "-p", "User", "-p", "Type", "-p", "Class",
             "-p", "Active", "-p", "LockedHint"], text=True).splitlines())
    before = state()
    assert before == {"User": "1000", "Type": "wayland", "Class": "user",
                      "Active": "yes", "LockedHint": "yes" if args.expect_locked else "no"}
    snapshot = subprocess.check_output(
        ["/tmp/pab-linux-media-a09ebd760230/linux-drm-inventory"], text=True, timeout=10)
    frames = [json.loads(line) for line in snapshot.splitlines() if line.startswith("{")]
    frame = next(v for v in frames if v.get("plane") == 34 and v.get("width", 0) > 64)
    libc = C.CDLL(None, use_errno=True)
    drm = C.CDLL("libdrm.so.2", use_errno=True)
    drm.drmModeGetFB2.argtypes = [C.c_int, C.c_uint32]
    drm.drmModeGetFB2.restype = C.POINTER(FB2)
    drm.drmModeFreeFB2.argtypes = [C.POINTER(FB2)]
    drm.drmModeGetCrtc.argtypes = [C.c_int, C.c_uint32]
    drm.drmModeGetCrtc.restype = C.POINTER(CrtcPrefix)
    drm.drmModeFreeCrtc.argtypes = [C.POINTER(CrtcPrefix)]
    drm.drmPrimeHandleToFD.argtypes = [C.c_int, C.c_uint32, C.c_uint32, C.POINTER(C.c_int)]
    drm.drmPrimeHandleToFD.restype = C.c_int
    drm.drmIoctl.argtypes = [C.c_int, C.c_ulong, C.c_void_p]
    drm.drmIoctl.restype = C.c_int
    drm.drmIsMaster.argtypes = [C.c_int]
    drm.drmIsMaster.restype = C.c_int

    captured = False
    pixels_read = False
    def inspect(fd):
        nonlocal captured, pixels_read
        report = {"master_before": bool(drm.drmIsMaster(fd))}
        crtc = drm.drmModeGetCrtc(fd, frame["crtc"])
        assert crtc, C.get_errno()
        try:
            current_fb = crtc.contents.buffer_id
        finally:
            drm.drmModeFreeCrtc(crtc)
        report["fb"] = current_fb
        fb = drm.drmModeGetFB2(fd, current_fb)
        report["getfb2_errno"] = C.get_errno() if not fb else 0
        if not fb:
            return report
        handles = set(int(h) for h in fb.contents.handles if h)
        try:
            value = fb.contents
            assert value.fb_id == current_fb and 64 < value.width <= 8192
            assert 64 < value.height <= 8192 and len(handles) == 1
            report.update(width=value.width, height=value.height, fourcc=value.pixel_format,
                          new_gem_handles=sorted(handles))
            exports = []
            for handle in sorted(handles):
                dma = C.c_int(-1)
                result = drm.drmPrimeHandleToFD(fd, handle, os.O_CLOEXEC, C.byref(dma))
                item = {"errno": C.get_errno() if result else 0, "exported": result == 0}
                if dma.value >= 0:
                    try:
                        item["fd_target"] = os.readlink(f"/proc/self/fd/{dma.value}")
                        item["size"] = os.fstat(dma.value).st_size
                        if args.pixel_reader and not captured:
                            captured = True
                            child = subprocess.run([str(args.pixel_reader), str(dma.value),
                                str(value.width), str(value.height), str(value.pixel_format),
                                str(value.pitches[0]), str(value.offsets[0]), str(value.modifier),
                                str(args.output)], pass_fds=(dma.value,), text=True,
                                capture_output=True, timeout=15)
                            item["pixel_reader"] = {"exit_code": child.returncode,
                                "stdout": child.stdout[-2000:], "stderr": child.stderr[-2000:]}
                            pixels_read = child.returncode == 0
                    finally:
                        os.close(dma.value)
                exports.append(item)
            report["exports"] = exports
        finally:
            errors = []
            for handle in sorted(handles):
                request = GemClose(handle, 0)
                result = drm.drmIoctl(fd, 0x40086409, C.byref(request))  # DRM_IOCTL_GEM_CLOSE
                if result:
                    errors.append({"handle": handle, "errno": C.get_errno()})
            drm.drmModeFreeFB2(fb)
            report["gem_close_errors"] = errors
            report["master_after"] = bool(drm.drmIsMaster(fd))
            assert not errors, report
        return report

    independent = os.open("/dev/dri/card0", os.O_RDONLY | os.O_CLOEXEC)
    try:
        own_report = inspect(independent)
    finally:
        os.close(independent)
    reports = []
    for process in pathlib.Path("/proc").iterdir():
        if not process.name.isdecimal():
            continue
        try:
            if process.stat().st_uid != 1000 or (process / "comm").read_text().strip() != "gnome-shell":
                continue
            if "user-1000.slice" not in (process / "cgroup").read_text():
                continue
            descriptors = [int(p.name) for p in (process / "fd").iterdir()
                           if p.is_symlink() and os.readlink(p) == "/dev/dri/card0"]
        except OSError:
            continue
        pidfd = libc.syscall(434, int(process.name), 0)  # x86_64 pidfd_open
        assert pidfd >= 0, C.get_errno()
        try:
            for original in descriptors[:4]:
                duplicate = libc.syscall(438, pidfd, original, 0)  # pidfd_getfd
                assert duplicate >= 0, C.get_errno()
                try:
                    reports.append({"pid": int(process.name), "original_fd": original,
                                    **inspect(duplicate)})
                finally:
                    os.close(duplicate)
        finally:
            os.close(pidfd)
    assert reports, "No matching compositor descriptor"
    assert state() == before
    print(json.dumps({"session": session, "kernel": platform.release(), "fb": frame["fb"],
                      "independent": own_report, "compositor": reports,
                      "session_unchanged": True, "locked": args.expect_locked,
                      "pixels_read": pixels_read, "modeset": False}))


if __name__ == "__main__":
    main()
