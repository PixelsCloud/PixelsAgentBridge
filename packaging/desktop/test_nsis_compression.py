"""Check actual runtime payloads using the installer's Windows ZIP extractor."""
from hashlib import sha256
from pathlib import Path
from tempfile import TemporaryDirectory
import subprocess
import zipfile

from build_nsis import ROOT, write_stored_runtime


def main():
    build = (ROOT / '.build').resolve()
    with TemporaryDirectory(prefix='nsis-runtime-test-', dir=build) as temporary:
        stage = Path(temporary).resolve()
        assert stage.is_relative_to(build)
        count = 0
        for runtime in ('browser', 'document'):
            source_path = build / f'{runtime}-runtime/{runtime}-runtime-windows.zip'
            before = sha256(source_path.read_bytes()).digest()
            stored_path = stage / f'{runtime}.zip'
            write_stored_runtime(source_path.read_bytes(), stored_path)
            with zipfile.ZipFile(source_path) as source, zipfile.ZipFile(stored_path) as stored:
                assert source.namelist() == stored.namelist()
                assert stored.testzip() is None
                for member in source.infolist():
                    actual = stored.getinfo(member.filename)
                    assert actual.compress_type == zipfile.ZIP_STORED
                    assert (actual.date_time, actual.external_attr) == (member.date_time, member.external_attr)
            extracted = stage / runtime
            subprocess.run([
                'powershell.exe', '-NoProfile', '-NonInteractive', '-Command',
                'Expand-Archive -LiteralPath $env:PAB_TEST_RUNTIME_ZIP -DestinationPath $env:PAB_TEST_RUNTIME_OUT -ErrorAction Stop',
            ], env=environment(stored_path, extracted), check=True)
            with zipfile.ZipFile(source_path) as source:
                files = [member for member in source.infolist() if not member.is_dir()]
                assert len(files) == sum(path.is_file() for path in extracted.rglob('*'))
                for member in files:
                    assert sha256(source.read(member)).digest() == sha256((extracted / member.filename).read_bytes()).digest(), member.filename
                count += len(files)
            assert sha256(source_path.read_bytes()).digest() == before
        print(f'PASS: {count} runtime files match after Windows PowerShell extraction; metadata and shared archives preserved')


def environment(archive, destination):
    import os
    return dict(os.environ, PAB_TEST_RUNTIME_ZIP=str(archive), PAB_TEST_RUNTIME_OUT=str(destination))


if __name__ == '__main__':
    main()
