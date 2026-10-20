#!/usr/bin/env python3
"""Verify public MCP input and JPEG capture after every own Activity is destroyed."""
import argparse
import base64
import hashlib
import io
import json
from pathlib import Path
import re
import time
import uuid

from PIL import Image

from android_public import PublicMcp
from android_scenarios import AndroidTasks
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def own_activity_count(serial):
    output = adb(serial, 'shell', 'dumpsys', 'activity', 'activities')
    return sum(bool(re.search(r'Hist\s+#\d+', line)) and PACKAGE + '/' in line
        for line in output.splitlines())


def foreground_service_count(serial):
    output = adb(serial, 'shell', 'dumpsys', 'activity', 'services', PACKAGE)
    chunks = re.split(r'(?=\* ServiceRecord\{)', output)
    selected = [chunk for chunk in chunks if PACKAGE + '/com.pixels.agent.ConnectionService'
        in chunk.splitlines()[0]]
    return sum('isForeground=true' in chunk for chunk in selected)


def layout_fingerprint(screen):
    # Retain a digest only, never launcher/app-list text or screen contents.
    payload = json.dumps({'package':screen.get('package_name'), 'elements':[
        {key:node.get(key) for key in ('role', 'bounds', 'clickable')}
        for node in screen['elements']]}, sort_keys=True).encode()
    return hashlib.sha256(payload).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--public-run-id', required=True, type=uuid.UUID)
    args = parser.parse_args()
    root = ROOT / '.build/android/public' / str(args.public_run_id)
    session = json.loads((root / 'session.json').read_text(encoding='utf-8'))
    assert installed_apk_sha256(args.serial) == session['apk_sha256']
    assert not adb(args.serial, 'reverse', '--list').strip()
    directory = root / ('background-' + str(uuid.uuid4()))
    directory.mkdir()
    report = {'status':'failed', 'provenance':{'apk_sha256':session['apk_sha256'],
        'serial':args.serial, 'public_server':True, 'forced_relay':True}, 'samples':[]}
    home = adb(args.serial, 'shell', 'cmd', 'package', 'resolve-activity', '--brief',
        '-a', 'android.intent.action.MAIN', '-c', 'android.intent.category.HOME')
    launcher = home.strip().splitlines()[-1].split('/')[0]
    assert re.fullmatch(r'[a-zA-Z][a-zA-Z0-9_.]+', launcher) and launcher != PACKAGE
    client = PublicMcp(root / 'mcp.toml', session['device_code'], True,
        directory / 'mcp.log', root / 'pab-mcp.exe')
    try:
        connected, raw = client.connect()
        assert connected['connected'] and not raw.get('isError')
        report['initial_own_activity_count'] = own_activity_count(args.serial)
        assert report['initial_own_activity_count'] > 0, 'Start from the normal visible home'
        # Setup destroys this app's task, then immediately finishes the replacement.
        # The ordinary ConnectionService remains the sole connection owner.
        adb(args.serial, 'shell', 'am', 'start', '-f', '0x10008000', '-n',
            f'{PACKAGE}/com.pixels.agent.fixture.AutomationFixtureActivity',
            '--ez', 'finish_fixture', 'true')
        deadline = time.monotonic() + 8
        while own_activity_count(args.serial):
            assert time.monotonic() < deadline, 'An own Activity remained after task destruction'
            time.sleep(.2)
        report['all_own_activities_destroyed_before'] = True
        tasks = AndroidTasks(client, PACKAGE, directory / 'operation-trace.jsonl')
        tasks.call({'action':'navigate', 'destination':'home'})
        started = time.monotonic()
        for index in range(19):
            time.sleep(max(0, started + index * 10 - time.monotonic()))
            sample_start = time.monotonic() - started if index else 0
            tasks.call({'action':'navigate', 'destination':'recents'})
            time.sleep(.4)
            recents, _ = tasks.call({'action':'observe'})
            tasks.call({'action':'navigate', 'destination':'home'})
            time.sleep(.4)
            home, _ = tasks.call({'action':'observe'})
            screen = home['observation']
            assert screen['package_name'] == launcher
            assert layout_fingerprint(recents['observation']) != layout_fingerprint(screen)
            _, raw = tasks.call({'action':'screenshot'})
            item = next(value for value in raw['content'] if value['type'] == 'image')
            payload = base64.b64decode(item['data'], validate=True)
            with Image.open(io.BytesIO(payload)) as image:
                assert image.format == 'JPEG' and image.width > 0 and image.height > 0
                image.load()
                dimensions = [image.width, image.height]
            count = foreground_service_count(args.serial)
            assert count == 1 and own_activity_count(args.serial) == 0
            report['samples'].append({'index':index, 'elapsed_seconds':round(sample_start, 3),
                'observe':True, 'input_postcondition':True, 'jpeg_decoded':True,
                'image_dimensions':dimensions, 'foreground_service_count':count})
            print(f'Public background sample {index + 1}/19 passed', flush=True)
        report['all_own_activities_destroyed_after'] = own_activity_count(args.serial) == 0
        assert report['samples'][-1]['elapsed_seconds'] >= 180
        report['status'] = 'passed'
    finally:
        client.close()
        (directory / 'result.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
        print(json.dumps({'directory':str(directory), 'status':report['status']}), flush=True)


if __name__ == '__main__':
    main()
