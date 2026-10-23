"""Run desktop unit tests in an explicitly selected Windows user session.

The parent must be the prepared LocalSystem test service. Native acceptance
tests remain in that service; only desktop unit tests use the selected user.
No credentials or inherited service environment are passed to the user process.
"""
from pathlib import Path
import json
import os
import subprocess
import sys


TOOLCHAIN_ENV = {'PATH', 'INCLUDE', 'LIB', 'LIBPATH', 'CMAKE', 'CMAKE_GENERATOR',
                 'CMAKE_BUILD_PARALLEL_LEVEL', 'CMAKE_BUILD_PARALLEL', '_CL_',
                 'PAB_CLIENT_DEFAULTS_FILE', 'TOKIO_WORKER_THREADS'}


def run(command, directory, session):
    if os.name != 'nt' or type(session) is not int or session <= 0:
        raise ValueError('Select a positive Windows interactive session ID')
    import ctypes as c
    from ctypes import wintypes as w

    class Startup(c.Structure):
        _fields_ = [('cb', w.DWORD), ('reserved', w.LPWSTR), ('desktop', w.LPWSTR),
                    ('title', w.LPWSTR), ('x', w.DWORD), ('y', w.DWORD),
                    ('width', w.DWORD), ('height', w.DWORD), ('chars_x', w.DWORD),
                    ('chars_y', w.DWORD), ('fill', w.DWORD), ('flags', w.DWORD),
                    ('show', w.WORD), ('reserved_size', w.WORD), ('reserved_ptr', c.c_void_p),
                    ('stdin', w.HANDLE), ('stdout', w.HANDLE), ('stderr', w.HANDLE)]

    class Process(c.Structure):
        _fields_ = [('process', w.HANDLE), ('thread', w.HANDLE), ('pid', w.DWORD), ('tid', w.DWORD)]

    kernel = c.WinDLL('kernel32', use_last_error=True)
    wts = c.WinDLL('wtsapi32', use_last_error=True)
    userenv = c.WinDLL('userenv', use_last_error=True)
    advapi = c.WinDLL('advapi32', use_last_error=True)
    wts.WTSQueryUserToken.argtypes = [w.ULONG, c.POINTER(w.HANDLE)]
    userenv.CreateEnvironmentBlock.argtypes = [c.POINTER(c.c_void_p), w.HANDLE, w.BOOL]
    userenv.DestroyEnvironmentBlock.argtypes = [c.c_void_p]
    advapi.CreateProcessAsUserW.argtypes = [w.HANDLE, w.LPCWSTR, w.LPWSTR, c.c_void_p,
        c.c_void_p, w.BOOL, w.DWORD, c.c_void_p, w.LPCWSTR, c.POINTER(Startup), c.POINTER(Process)]
    kernel.WaitForSingleObject.argtypes = [w.HANDLE, w.DWORD]
    kernel.GetExitCodeProcess.argtypes = [w.HANDLE, c.POINTER(w.DWORD)]
    kernel.TerminateProcess.argtypes = [w.HANDLE, w.UINT]
    kernel.CloseHandle.argtypes = [w.HANDLE]
    directory = directory.resolve()
    directory.mkdir(parents=True, exist_ok=False)
    request = directory / 'request.json'
    request.write_text(json.dumps({'command': command, 'cwd': str(Path.cwd()),
        'session': session, 'environment': {k: v for k, v in os.environ.items() if k.upper() in TOOLCHAIN_ENV}}), encoding='utf-8')
    token, block, process = w.HANDLE(), c.c_void_p(), Process()
    try:
        if not wts.WTSQueryUserToken(session, c.byref(token)):
            raise c.WinError(c.get_last_error())
        if not userenv.CreateEnvironmentBlock(c.byref(block), token, False):
            raise c.WinError(c.get_last_error())
        startup = Startup(cb=c.sizeof(Startup), desktop='winsta0\\default')
        argv = c.create_unicode_buffer(subprocess.list2cmdline([sys.executable, str(Path(__file__).resolve()), '--child', str(request)]))
        if not advapi.CreateProcessAsUserW(token, sys.executable, argv, None, None, False,
                0x08000000 | 0x00000400, block, str(Path.cwd()), c.byref(startup), c.byref(process)):
            raise c.WinError(c.get_last_error())
        if kernel.WaitForSingleObject(process.process, 1830 * 1000) != 0:
            kernel.TerminateProcess(process.process, 1)
            raise TimeoutError('Desktop unit test process did not finish')
        code = w.DWORD()
        if not kernel.GetExitCodeProcess(process.process, c.byref(code)):
            raise c.WinError(c.get_last_error())
        receipt = json.loads((directory / 'result.json').read_text(encoding='utf-8'))
        if receipt['session'] != session or receipt['returncode'] != code.value:
            raise RuntimeError('Desktop test identity or completion receipt differs')
        return subprocess.CompletedProcess(command, code.value,
            (directory / 'stdout.log').read_text(encoding='utf-8'),
            (directory / 'stderr.log').read_text(encoding='utf-8'))
    finally:
        for handle in (process.thread, process.process, token):
            if handle: kernel.CloseHandle(handle)
        if block: userenv.DestroyEnvironmentBlock(block)


def child(request):
    import ctypes
    from ctypes import wintypes
    value = json.loads(request.read_text(encoding='utf-8'))
    session = wintypes.DWORD()
    if not ctypes.windll.kernel32.ProcessIdToSessionId(os.getpid(), ctypes.byref(session)):
        raise ctypes.WinError()
    if session.value != value['session']:
        raise RuntimeError('Desktop test started in a different Windows session')
    env = dict(os.environ, **value['environment'])
    result = subprocess.run(value['command'], cwd=value['cwd'], env=env,
                            capture_output=True, encoding='utf-8', errors='replace', timeout=1800)
    (request.parent / 'stdout.log').write_text(result.stdout, encoding='utf-8')
    (request.parent / 'stderr.log').write_text(result.stderr, encoding='utf-8')
    (request.parent / 'result.json').write_text(json.dumps({'session': session.value, 'returncode': result.returncode}), encoding='utf-8')
    return result.returncode


if __name__ == '__main__':
    if len(sys.argv) != 3 or sys.argv[1] != '--child':
        raise SystemExit('Use this helper through smoke/executor.py')
    raise SystemExit(child(Path(sys.argv[2])))
