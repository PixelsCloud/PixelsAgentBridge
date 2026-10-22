"""Memory ownership for a newly spawned Windows acceptance process only."""
import ctypes
from ctypes import wintypes
import os


class _IoCounters(ctypes.Structure):
    _fields_ = [(name, ctypes.c_ulonglong) for name in (
        'ReadOperationCount', 'WriteOperationCount', 'OtherOperationCount',
        'ReadTransferCount', 'WriteTransferCount', 'OtherTransferCount',
    )]


class _BasicLimitInformation(ctypes.Structure):
    _fields_ = [
        ('PerProcessUserTimeLimit', ctypes.c_longlong),
        ('PerJobUserTimeLimit', ctypes.c_longlong),
        ('LimitFlags', wintypes.DWORD),
        ('MinimumWorkingSetSize', ctypes.c_size_t),
        ('MaximumWorkingSetSize', ctypes.c_size_t),
        ('ActiveProcessLimit', wintypes.DWORD),
        ('Affinity', ctypes.c_size_t),
        ('PriorityClass', wintypes.DWORD),
        ('SchedulingClass', wintypes.DWORD),
    ]


class _ExtendedLimitInformation(ctypes.Structure):
    _fields_ = [
        ('BasicLimitInformation', _BasicLimitInformation),
        ('IoInfo', _IoCounters),
        ('ProcessMemoryLimit', ctypes.c_size_t),
        ('JobMemoryLimit', ctypes.c_size_t),
        ('PeakProcessMemoryUsed', ctypes.c_size_t),
        ('PeakJobMemoryUsed', ctypes.c_size_t),
    ]


class OwnedProcessMemoryLimit:
    """A job handle tied to a Popen child, never an arbitrary installed PID.

    Closing the handle also terminates a surviving owned child. A Windows job
    limits private committed process memory, not the host filesystem cache.
    """

    def __init__(self, process, limit_bytes):
        if os.name != 'nt' or process.poll() is not None:
            raise RuntimeError('Requires a live, newly created Windows Popen child')
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.CreateJobObjectW.argtypes = [ctypes.c_void_p, wintypes.LPCWSTR]
        kernel.CreateJobObjectW.restype = wintypes.HANDLE
        kernel.SetInformationJobObject.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD]
        kernel.SetInformationJobObject.restype = wintypes.BOOL
        kernel.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
        kernel.AssignProcessToJobObject.restype = wintypes.BOOL
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel.CloseHandle.restype = wintypes.BOOL
        self._kernel = kernel
        self._handle = kernel.CreateJobObjectW(None, None)
        if not self._handle:
            raise ctypes.WinError(ctypes.get_last_error())
        information = _ExtendedLimitInformation()
        # PROCESS_MEMORY | KILL_ON_JOB_CLOSE, affecting only this owned job.
        information.BasicLimitInformation.LimitFlags = 0x100 | 0x2000
        information.ProcessMemoryLimit = limit_bytes
        try:
            if not kernel.SetInformationJobObject(self._handle, 9, ctypes.byref(information), ctypes.sizeof(information)):
                raise ctypes.WinError(ctypes.get_last_error())
            if not kernel.AssignProcessToJobObject(self._handle, wintypes.HANDLE(int(process._handle))):
                raise ctypes.WinError(ctypes.get_last_error())
        except Exception:
            self.close()
            raise
        self.limit_bytes = limit_bytes

    def close(self):
        if self._handle:
            self._kernel.CloseHandle(self._handle)
            self._handle = None
