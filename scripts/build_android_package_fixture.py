#!/usr/bin/env python3
"""Build owned, permission-free APK fixtures without installing them through ADB.

Two processor cores, sequential tools, API 28 minimum. The remote package tools
perform every install/update/removal during acceptance. Keys are fixture-only.
"""
from __future__ import annotations
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import zipfile

import build_android as build

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / '.build/android/app-management/fixtures'
PACKAGE = 'com.pixels.acceptance.packagefixture'


def main():
    build.MAX_JOBS = 2
    build.limit_processors()
    env = build.environment()
    env['JAVA_TOOL_OPTIONS'] = '-XX:ActiveProcessorCount=2'
    sdk = Path(env['ANDROID_HOME'])
    tools = sdk / 'build-tools/36.0.0'
    java = Path(env['JAVA_HOME']) / 'bin'
    android = sdk / 'platforms/android-36/android.jar'
    OUT.mkdir(parents=True, exist_ok=True)

    def run(args):
        subprocess.run([str(arg) for arg in args], env=env, cwd=OUT, check=True,
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    classes = OUT / 'classes'
    classes.mkdir(exist_ok=True)
    run([java / 'javac.exe', '-source', '17', '-target', '17', '-classpath', android,
         '-d', classes, ROOT / 'acceptance/fixtures/android-package/MainActivity.java'])
    dex = OUT / 'dex'
    dex.mkdir(exist_ok=True)
    run([tools / 'd8.bat', '--min-api', '28', '--lib', android, '--output', dex,
         classes / 'com/pixels/acceptance/packagefixture/MainActivity.class'])
    for alias in ('fixture', 'different'):
        key = OUT / (alias + '.jks')
        if not key.exists():
            run([java / 'keytool.exe', '-genkeypair', '-keystore', key, '-storepass', 'fixture-test-only',
                 '-keypass', 'fixture-test-only', '-alias', alias, '-dname', 'CN=Pixels Acceptance Fixture',
                 '-keyalg', 'RSA', '-validity', '3650'])
    artifacts = []
    for version, alias in ((1, 'fixture'), (2, 'fixture'), (3, 'different')):
        directory = OUT / f'v{version}'
        directory.mkdir(exist_ok=True)
        manifest = directory / 'AndroidManifest.xml'
        manifest.write_text(f'''<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="{PACKAGE}" android:versionCode="{version}" android:versionName="{version}">
<uses-sdk android:minSdkVersion="28" android:targetSdkVersion="36"/>
<application android:label="Pixels Package Fixture" android:allowBackup="false" android:theme="@android:style/Theme.Material.Light.NoActionBar">
<activity android:name=".MainActivity" android:exported="true"><intent-filter><action android:name="android.intent.action.MAIN"/><category android:name="android.intent.category.LAUNCHER"/></intent-filter></activity>
</application></manifest>''', encoding='utf-8')
        unsigned = directory / 'unsigned.apk'
        run([tools / 'aapt2.exe', 'link', '-I', android, '--manifest', manifest, '-o', unsigned])
        with zipfile.ZipFile(unsigned, 'a') as archive:
            archive.write(dex / 'classes.dex', 'classes.dex')
        aligned = directory / 'aligned.apk'
        run([tools / 'zipalign.exe', '-f', '4', unsigned, aligned])
        apk = OUT / f'fixture-v{version}.apk'
        run([tools / 'apksigner.bat', 'sign', '--ks', OUT / (alias + '.jks'), '--ks-key-alias', alias,
             '--ks-pass', 'pass:fixture-test-only', '--key-pass', 'pass:fixture-test-only', '--out', apk, aligned])
        run([tools / 'apksigner.bat', 'verify', apk])
        artifacts.append({'version_code': version, 'path': str(apk), 'sha256': hashlib.sha256(apk.read_bytes()).hexdigest(), 'bytes': apk.stat().st_size, 'different_signer': alias != 'fixture'})
    (OUT / 'artifacts.json').write_text(json.dumps({'package_id': PACKAGE, 'artifacts': artifacts,
        'minimum_sdk': 28, 'build_jobs': 2, 'installed_via_adb': False}, indent=2), encoding='utf-8')
    print(json.dumps({'status': 'passed', 'artifacts': len(artifacts), 'installed_via_adb': False}))


if __name__ == '__main__':
    main()
