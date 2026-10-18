"""Exercise real NSIS initialization without installing or modifying Pixels."""
from contextlib import contextmanager
from pathlib import Path
from tempfile import TemporaryDirectory
import json
import os
import re
import subprocess
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
WINDOWS = ROOT / 'packaging/desktop/windows'
COMPILER = ROOT / 'tools/nsis/makensis.exe'
POWERSHELL = 'powershell.exe'


def run(command, **kwargs):
    return subprocess.run([str(part) for part in command], capture_output=True, text=True,
                          encoding='utf-8', errors='replace', timeout=90, **kwargs)


@contextmanager
def held_object(name, semaphore=False):
    # Hold only a disposable named object. A semaphore with the same name makes
    # CreateMutexW fail with real ERROR_INVALID_HANDLE (6), without changing ACLs.
    expression = ('[Threading.Semaphore]::new(0,1,$name)' if semaphore else
                  '[Threading.Mutex]::new($false,$name)')
    script = (f"$ErrorActionPreference='Stop'; $name='{name}'; $held={expression}; "
              "[Console]::Out.WriteLine('READY'); [Console]::Out.Flush(); "
              "[Console]::In.ReadLine() | Out-Null; $held.Dispose()")
    process = subprocess.Popen([POWERSHELL, '-NoProfile', '-NonInteractive', '-Command', script],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               text=True, encoding='utf-8', errors='replace')
    try:
        assert process.stdout.readline().strip() == 'READY', 'Named object fixture failed'
        yield
    finally:
        process.communicate('\n', timeout=15)
        assert process.returncode == 0


def compile_fixture(stage, name, log_root):
    executable = stage / (name + '.exe')
    source = stage / (name + '.nsi')
    mutex = 'Local\\PixelsSetupDiagnostics-' + uuid.uuid4().hex
    recovery = re.search(r'Function PabRunSetupRecovery\n.*?FunctionEnd',
        (WINDOWS / 'setup.nsi').read_text(encoding='utf-8'), re.S).group()
    # Scope real recovery to the disposable old-installer fixture only.
    recovery = recovery.replace('-CallerPid $0', f'-PreviousSetupPath "{stage / "old-setup.exe"}" -CallerPid $0')
    source.write_text(f'''Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
Name "Pixels isolated setup diagnostics"
OutFile "{executable}"
RequestExecutionLevel user
InstallDir "$EXEDIR"
!define APP_VERSION "0.0.0"
!define BUILD_PROFILE "test"
!define PAB_SETUP_MUTEX_NAME "{mutex}"
!define PAB_SETUP_LOG_ROOT "{log_root}"
!define PAB_SETUP_FALLBACK_LOG_ROOT "{stage / 'fallback logs'}"
!define PAYLOAD_DIR "{WINDOWS}"
!define PAB_SETUP_RECOVERY_FUNCTION PabRunSetupRecovery
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "SimpChinese"
LangString SetupAlreadyRunning ${{LANG_ENGLISH}} "Another setup is running."
LangString SetupAlreadyRunning ${{LANG_SIMPCHINESE}} "另一个安装程序正在运行。"
!include "{WINDOWS / 'setup-diagnostics.nsh'}"
{recovery}
Function .onInit
 !insertmacro PabInitializeSetup ""
FunctionEnd
Function un.onInit
 !insertmacro PabInitializeSetup "un."
FunctionEnd
Function .onGUIEnd
 Call PabCloseSetupLog
FunctionEnd
Function un.onGUIEnd
 Call un.PabCloseSetupLog
FunctionEnd
Section
 StrCpy $SetupPhase "completed"
 StrCpy $SetupMessage "fixture_install_succeeded"
 Call PabSetupLog
 WriteUninstaller "$EXEDIR\\fixture-uninstall.exe"
SectionEnd
Section "Uninstall"
 StrCpy $SetupPhase "completed"
 StrCpy $SetupMessage "fixture_uninstall_succeeded"
 Call un.PabSetupLog
SectionEnd
''', encoding='utf-8')
    result = run([COMPILER, '/V2', '/INPUTCHARSET', 'UTF8', source])
    assert result.returncode == 0, result.stdout + result.stderr
    return executable, mutex


def logs(directory):
    return {p: p.read_bytes() for p in directory.glob('*.log')}


def execute_fixture(executable, directory, expected_exit, expected_text, *args, environment=None):
    before = logs(directory)
    result = run([executable, '/S', *args], env=environment)
    after = logs(directory)
    assert result.returncode == expected_exit, (result.returncode, expected_exit, result.stdout, result.stderr,
        [(str(p), content.decode('utf-16' if not p.name.endswith('.powershell.log') else 'utf-8-sig', errors='replace'))
         for p, content in after.items() if p not in before])
    created = {p for p in set(after) - set(before) if not p.name.endswith('.powershell.log')}
    assert len(created) == 1, created
    for path, content in before.items():
        assert after[path] == content, f'Prior attempt was overwritten: {path}'
    path = created.pop()
    text = after[path].decode('utf-16')
    assert expected_text in text, text
    assert '[nsis] phase=' in text and 'Z [nsis]' in text, text
    return path


def powershell_logging(stage, shared_log):
    # Use the production logger and real Windows PowerShell 5.1 transcripts.
    # All output stays in the test workspace, including the legacy error alias.
    script = stage / 'logging-fixture.ps1'
    native = stage / 'native fixture 中文.ps1'
    native.write_text('''param([int]$PauseSeconds=0)
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
[Console]::Out.WriteLine('native_process_output')
[Console]::Error.WriteLine('native_process_error')
if($PauseSeconds){Start-Sleep -Seconds $PauseSeconds}
exit 7
''', encoding='utf-8-sig')
    script.write_text(f'''param([string]$LogPath,[switch]$Fail,[switch]$CheckTimeout)
$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
. '{WINDOWS / 'installer-logging.ps1'}'
Initialize-PabInstallerLog -Action 'test' -Path $LogPath
trap {{
 Write-PabInstallerFailure -Failure $_ -LegacyLog '{stage / 'legacy-error.log'}'
 Stop-PabInstallerLog
 exit 1
}}
Set-PabInstallerStage 'validate_inputs'
Write-Output 'transcript output 中文'
$nativeArguments = @('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File','{native}')
if($CheckTimeout){{
 $watch=[Diagnostics.Stopwatch]::StartNew()
 $timedOut=$false
 try {{ $null=Invoke-PabInstallerNativeCommand -FilePath 'powershell.exe' -ArgumentList ($nativeArguments + @('-PauseSeconds','20')) -TimeoutSeconds 1 }}
 catch {{ $timedOut=$_.Exception.Message -like '*exceeded 1 seconds*' }}
 if(-not $timedOut -or $watch.Elapsed.TotalSeconds -gt 8){{throw 'Installer command timeout was not bounded'}}
}}
$nativeExit = Invoke-PabInstallerNativeCommand -FilePath 'powershell.exe' -ArgumentList $nativeArguments
if($nativeExit -ne 7){{throw 'Native command exit code was lost'}}
if($Fail){{throw 'test installer failure'}}
Set-PabInstallerStage 'completed'
Write-PabInstallerLog 'success'
Stop-PabInstallerLog
''', encoding='utf-8-sig')
    prefix = [POWERSHELL, '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', script]
    result = run([*prefix, '-LogPath', shared_log, '-Fail', '-CheckTimeout'])
    assert result.returncode == 1, result.stdout + result.stderr
    first = shared_log.read_bytes()
    assert 'test installer failure' in first.decode('utf-16'), first.decode('utf-16') + result.stdout + result.stderr
    assert 'stage=validate_inputs' in first.decode('utf-16')
    assert 'native_output=native_process_error' in first.decode('utf-16')
    assert 'native_exit_code=7' in first.decode('utf-16')
    assert 'fixture_install_succeeded' in first.decode('utf-16'), 'NSIS and PowerShell log encodings diverged'
    transcript = Path(str(shared_log) + '.powershell.log').read_text(encoding='utf-8-sig')
    assert 'transcript output 中文' in transcript and 'native_process_output' in transcript, transcript
    assert 'native_process_error' in transcript, 'Native stderr was missing from the transcript'
    legacy = (stage / 'legacy-error.log').read_bytes()
    second = stage / 'successful-retry.log'
    result = run([*prefix, '-LogPath', second])
    assert result.returncode == 0, result.stdout + result.stderr
    assert 'success' in second.read_text(encoding='utf-16')
    assert shared_log.read_bytes() == first, 'Successful retry erased the failed attempt'
    assert (stage / 'legacy-error.log').read_bytes() == legacy, 'Successful retry erased error history'
    environment = os.environ.copy()
    environment['ProgramData'] = str(stage / 'isolated-program-data')
    environment['TEMP'] = environment['TMP'] = str(stage / 'isolated-temp')
    Path(environment['TEMP']).mkdir()
    directory = Path(environment['ProgramData']) / 'PixelsAgentBridge/logs/setup'
    for _ in range(2):
        before = logs(directory)
        result = run(prefix, env=environment)
        assert result.returncode == 0, result.stdout + result.stderr
        after = logs(directory)
        assert len(set(after) - set(before)) == 2, 'Each attempt needs its own log and transcript'
        assert all(after[path] == content for path, content in before.items())
    # A genuinely unusable primary path must fall back without changing the
    # installer result. Override both environment roots to keep the test isolated.
    blocked = stage / 'powershell-blocked-directory'
    blocked.write_text('not a directory')
    result = run([*prefix, '-LogPath', blocked / 'log.txt'], env=environment)
    assert result.returncode == 0, result.stdout + result.stderr
    fallback = Path(environment['TEMP']) / 'PixelsAgentBridge-InstallerLogs'
    assert len(list(fallback.glob('*.log'))) == 2, 'PowerShell log fallback was not used'


def validate_zip_entry(stage):
    environment = os.environ.copy()
    environment['ProgramData'] = str(stage / 'zip-program-data')
    # Invalid input fails before administration checks, registry access or file
    # replacement. Execute the actual install.ps1 trap, not just the helper.
    install_root = stage / 'zip-install-root'
    result = run([POWERSHELL, '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
        '-File', WINDOWS / 'install.ps1', '-ControlUrl', 'http://invalid.example',
        '-RelayUrl', 'https://invalid.example', '-InstallRoot', install_root,
        '-InstallerLogPath', stage / 'zip-validation.log'], env=environment)
    assert result.returncode != 0, result.stdout + result.stderr
    assert not install_root.exists(), 'Invalid input changed the installation directory'
    log = (stage / 'zip-validation.log').read_text(encoding='utf-16')
    assert 'stage=validate_inputs' in log and 'ControlUrl must use wss://' in log
    alias = Path(environment['ProgramData']) / 'PixelsAgentBridge/logs/install-error.log'
    assert 'ControlUrl must use wss://' in alias.read_text(encoding='utf-8-sig')


def force_upgrade(stage, executable, mutex, log_root):
    old = stage / 'old-setup.exe'
    marker = stage / 'old-ready.txt'
    child_marker = stage / 'old-child.txt'
    child = stage / 'old-helper.ps1'
    child.write_text(f"[IO.File]::WriteAllText('{child_marker}',[string]$PID); Start-Sleep -Seconds 60", encoding='utf-8-sig')
    environment = os.environ.copy()
    environment['ProgramData'] = str(stage / 'recovery-program-data')
    for mode in ('legacy', 'coordinator', 'uninstaller'):
        for path in (marker, child_marker):
            path.unlink(missing_ok=True)
        extra = f"System::Call 'kernel32::CreateMutexW(p0,i0,w\"{mutex}-v2\")p.r1'" if mode == 'coordinator' else ''
        output = stage / 'old-writer.exe' if mode == 'uninstaller' else old
        install_section = f' WriteUninstaller "{old}"\nSectionEnd\nSection "Uninstall"' if mode == 'uninstaller' else ''
        source = stage / 'old-setup.nsi'
        source.write_text(f'''Unicode true
Name "Pixels disposable previous setup"
OutFile "{output}"
RequestExecutionLevel user
LoadLanguageFile "${{NSISDIR}}\\Contrib\\Language files\\English.nlf"
VIProductVersion "0.0.0.0"
VIAddVersionKey /LANG=1033 "ProductName" "Pixels Agent Bridge"
VIAddVersionKey /LANG=1033 "FileDescription" "Pixels Agent Bridge test Installer"
VIAddVersionKey /LANG=1033 "FileVersion" "0.0.0"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Pixels disposable test fixture"
Section
{install_section}
 System::Call 'kernel32::CreateMutexW(p0,i0,w"{mutex}")p.r0'
 {extra}
 Exec '"$WINDIR\\Sysnative\\WindowsPowerShell\\v1.0\\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "{child}"'
 FileOpen $2 "{marker}" w
 FileWrite $2 "ready"
 FileClose $2
 Sleep 60000
SectionEnd
''', encoding='utf-8')
        compiled = run([COMPILER, '/V2', '/INPUTCHARSET', 'UTF8', source])
        assert compiled.returncode == 0, compiled.stdout + compiled.stderr
        if mode == 'uninstaller':
            old.unlink(missing_ok=True)
            generated = run([output, '/S'])
            assert generated.returncode == 0 and old.exists()
        blocker = subprocess.Popen([str(old), '/S', '_?=' + str(stage)] if mode == 'uninstaller' else [str(old), '/S'],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        child_pid = None
        try:
            deadline = time.monotonic() + 10
            while not (marker.exists() and child_marker.exists()):
                assert time.monotonic() < deadline and blocker.poll() is None, 'Old setup fixture did not start'
                time.sleep(.05)
            child_pid = int(child_marker.read_text(encoding='utf-8-sig'))
            try:
                execute_fixture(executable, log_root, 0, 'force_upgrade_recovery_exit=0', '/FORCE', environment=environment)
            except AssertionError as error:
                details = run([POWERSHELL, '-NoProfile', '-NonInteractive', '-Command',
                    f"Get-CimInstance Win32_Process -Filter 'ProcessId = {blocker.pid}' | Select-Object ProcessId,ParentProcessId,ExecutablePath | Format-List; [Diagnostics.FileVersionInfo]::GetVersionInfo('{old}') | Select-Object ProductName,FileDescription | Format-List"])
                raise AssertionError(str(error) + details.stdout + details.stderr) from error
            assert blocker.wait(timeout=5) != 0, 'Previous installer was not terminated'
            checked = run([POWERSHELL, '-NoProfile', '-NonInteractive', '-Command',
                f"if(Get-Process -Id {child_pid} -ErrorAction SilentlyContinue){{exit 1}}"])
            assert checked.returncode == 0, 'Previous PowerShell helper survived force upgrade'
        finally:
            if blocker.poll() is None:
                blocker.kill()
                blocker.wait(timeout=5)
            if child_pid:
                run([POWERSHELL, '-NoProfile', '-NonInteractive', '-Command',
                    f"Stop-Process -Id {child_pid} -Force -ErrorAction SilentlyContinue"])


def validate_powershell_syntax():
    for name in ('install.ps1', 'uninstall.ps1', 'installer-logging.ps1', 'installer-recovery.ps1'):
        script = ("$ErrorActionPreference='Stop'; $tokens=$null; $parseErrors=$null; "
            f"[void][Management.Automation.Language.Parser]::ParseFile('{WINDOWS / name}',"
            "[ref]$tokens,[ref]$parseErrors); if($parseErrors.Count){throw ($parseErrors|Out-String)}")
        result = run([POWERSHELL, '-NoProfile', '-NonInteractive', '-Command', script])
        assert result.returncode == 0, result.stdout + result.stderr


def compile_production_script(stage):
    payload = stage / 'payload'
    payload.mkdir()
    from build_nsis import CONTENTS
    for name in CONTENTS:
        (payload / name).write_bytes(b'compile-only fixture; never execute this installer')
    result = run([COMPILER, '/V2', '/INPUTCHARSET', 'UTF8', f'/DPAYLOAD_DIR={payload}',
        f'/DOUTPUT_FILE={stage / "production-compile-only.exe"}', '/DCONTROL_URL=wss://invalid.example/control',
        '/DRELAY_URL=https://invalid.example', f'/DAPP_ICON={ROOT / "apps/desktop/src-tauri/icons/icon.ico"}',
        '/DBUILD_PROFILE=test', '/DAPP_VERSION=0.0.0', WINDOWS / 'setup.nsi'])
    assert result.returncode == 0, result.stdout + result.stderr


def main():
    build = (ROOT / '.build').resolve()
    build.mkdir(exist_ok=True)
    with TemporaryDirectory(prefix='installer-diagnostics-', dir=build) as temporary:
        stage = Path(temporary).resolve()
        assert stage.is_relative_to(build)
        log_root = stage / 'logs spaced 中文'
        executable, mutex = compile_fixture(stage, 'diagnostics', log_root)
        success = execute_fixture(executable, log_root, 0, 'fixture_install_succeeded')
        with held_object(mutex):
            execute_fixture(executable, log_root, 2, 'setup_already_running win32_error=183')
            execute_fixture(stage / 'fixture-uninstall.exe', log_root, 2, 'setup_already_running', '_?=' + str(stage))
        with held_object(mutex, semaphore=True):
            execute_fixture(executable, log_root, 0, 'legacy_lock_unavailable win32_error=6')
            execute_fixture(stage / 'fixture-uninstall.exe', log_root, 0, 'legacy_lock_unavailable win32_error=6', '_?=' + str(stage))
        with held_object(mutex + '-v2'):
            execute_fixture(executable, log_root, 2, 'setup_already_running coordinator')
        with held_object(mutex + '-v2', semaphore=True):
            execute_fixture(executable, log_root, 2, 'coordinator_initialization_failed win32_error=6')
        execute_fixture(executable, log_root, 0, 'fixture_install_succeeded')
        execute_fixture(stage / 'fixture-uninstall.exe', log_root, 0, 'fixture_uninstall_succeeded', '_?=' + str(stage))
        blocked = stage / 'blocked-log-root'
        blocked.write_text('not a directory')
        fallback, _ = compile_fixture(stage, 'fallback', blocked)
        execute_fixture(fallback, stage / 'fallback logs', 0, 'fixture_install_succeeded')
        powershell_logging(stage, success)
        force_upgrade(stage, executable, mutex, log_root)
        validate_zip_entry(stage)
        validate_powershell_syntax()
        compile_production_script(stage)
        print(json.dumps({'passed': True, 'checks': ['NSIS real mutex contention', 'NSIS real initialization error code',
            'broken legacy lock coordinator fallback', 'forced legacy installer takeover',
            'forced coordinator takeover', 'forced real NSIS uninstaller takeover',
            'old installer child processes terminated', 'bounded hung native command',
            'installer and uninstaller logs', 'unique logs across successful retries', 'unwritable directory fallback',
            'UTF-16 shared log', 'PowerShell transcript with native output', 'legacy errors retained',
            'PowerShell default unique logs and fallback', 'real ZIP entry failure diagnostics',
            'Windows PowerShell 5.1 syntax',
            'complete production NSIS script compilation'], 'local_installation_modified': False}))


if __name__ == '__main__':
    main()
