"""Build both generated-pixel geometry probes serially on one CPU.

Run the produced presenter probe in an unlocked graphical session, and the
encoder probe in its output directory. The latter needs NVIDIA NVENC; validate
its streams with verify_windows_odd_encoder.py. No installed app is replaced.
"""
import ctypes
from ctypes import wintypes
from pathlib import Path
from build_dxgi_probe import build_probe

kernel = ctypes.WinDLL('kernel32', use_last_error=True)
kernel.GetCurrentProcess.restype = wintypes.HANDLE
kernel.GetProcessAffinityMask.argtypes = [wintypes.HANDLE, ctypes.POINTER(ctypes.c_size_t), ctypes.POINTER(ctypes.c_size_t)]
kernel.SetProcessAffinityMask.argtypes = [wintypes.HANDLE, ctypes.c_size_t]
handle = kernel.GetCurrentProcess()
allowed, system = ctypes.c_size_t(), ctypes.c_size_t()
if not kernel.GetProcessAffinityMask(handle, ctypes.byref(allowed), ctypes.byref(system)):
    raise ctypes.WinError(ctypes.get_last_error())
if not kernel.SetProcessAffinityMask(handle, allowed.value & -allowed.value):
    raise ctypes.WinError(ctypes.get_last_error())

libraries = tuple(name + '.lib' for name in ('dxguid', 'gdi32', 'mfplat', 'mfuuid', 'wmcodecdspuuid', 'ole32', 'shcore', 'shell32', 'advapi32'))
vendor = Path(__file__).resolve().parents[2]/'crates/desktop-media/native/vendor'
for kind in ('presenter', 'encoder'):
    build_probe(f'windows_odd_{kind}.cpp', f'windows-odd-{kind}', vendor, libraries)
