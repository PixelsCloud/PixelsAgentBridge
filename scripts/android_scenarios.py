"""Business postcondition checks over the real Android MCP channel; never uses ADB input."""
from __future__ import annotations

import base64
from collections import Counter
import io
import json
from pathlib import Path
import re
import time
import uuid


class UnconfirmedOutcome(RuntimeError):
    """A dispatch may have happened; the campaign must stop rather than send more input."""


class ControlMissingBeforeDispatch(RuntimeError):
    """An observed control disappeared before this helper dispatched any input."""


class AndroidTasks:
    def __init__(self, mcp, package: str, trace: Path):
        self.mcp = mcp
        self.package = package
        self.trace = trace
        self.calls = 0
        self.last_input_request = None
        self.last_input_command = None
        self.paths = Counter()

    def call(self, command, *, expected_error=None, allow_pre_dispatch_errors=()):
        request = str(uuid.uuid4())
        started = time.monotonic()
        value, raw = self.mcp.mobile(request, command)
        deadline = started + 32
        while value['operation']['state'] == 'running' and time.monotonic() < deadline:
            time.sleep(0.1)
            value, raw = self.mcp.mobile(request, operation='get')
        result = value['operation']
        path = value.get('connection_path') or 'unknown'
        self.paths[path] += 1
        self.calls += 1
        event = {'request_id': request, 'action': command['action'], 'state': result['state'],
                 'error': result.get('error'), 'action_dispatched': result['action_dispatched'],
                 'elapsed_ms': round((time.monotonic() - started) * 1000), 'connection_path':path}
        with self.trace.open('a', encoding='utf-8') as output:
            output.write(json.dumps(event) + '\n')
        if result['state'] in ('running', 'unconfirmed'):
            raise UnconfirmedOutcome(f'Query original request before further input: {request}')
        if expected_error is not None:
            assert result['state'] == 'failed' and result.get('error') == expected_error, event
        elif result['state'] != 'completed' and not (
            result['state'] == 'failed' and result.get('error') in allow_pre_dispatch_errors
            and not result['action_dispatched']):
            raise RuntimeError(f"Android task stopped without replaying input: {event}")
        return result, raw

    def query(self, *, text=None, role=None, root=None):
        for _ in range(3):
            result, _ = self.call({'action': 'query', 'root_ref': root,
                'text_contains': text, 'role': role, 'max_nodes': 128, 'max_depth': 32},
                allow_pre_dispatch_errors=('target_stale',))
            if result['state'] == 'completed':
                return result['observation']
            # Read-only observation raced a window transition. No input was
            # dispatched; retry only the same observation, never a mutation.
            time.sleep(.2)
        raise RuntimeError('Observation stayed stale across three window snapshots')

    @staticmethod
    def named(screen, name):
        return next((node for node in screen['elements'] if node.get('name') == name), None)

    @staticmethod
    def has(screen, text):
        return any(text in (node.get('name') or '') for node in screen['elements'])

    def act(self, name, action='click', **fields):
        # Only explicit target_stale before dispatch permits a fresh observation and request.
        for _ in range(3):
            time.sleep(0.2)
            screen = self.query(text=name)
            node = self.named(screen, name)
            if node is None:
                raise ControlMissingBeforeDispatch(f'Expected test control missing: {name}')
            if action == 'click' and not node['clickable']:
                # Compose can expose a button's text as a separate virtual child.
                # Select its unique enclosing native Button from one fresh snapshot.
                # The input still targets an explicit reference; the backend never retargets.
                screen = self.query()
                label = self.named(screen, name)
                if label is None:
                    raise ControlMissingBeforeDispatch('Control changed while resolving its button')
                bounds = label['bounds']
                candidates = [item for item in screen['elements'] if item['clickable']
                    and item['role'] in ('android.widget.Button', 'android.view.View')
                    and item['bounds']['left'] <= bounds['left']
                    and item['bounds']['top'] <= bounds['top']
                    and item['bounds']['right'] >= bounds['right']
                    and item['bounds']['bottom'] >= bounds['bottom']]
                if len(candidates) != 1:
                    raise RuntimeError('The text does not identify one visible native button')
                node = candidates[0]
            command = {'action': action, 'element_ref': node['element_ref'], **fields}
            request = str(uuid.uuid4())
            value, _ = self.mcp.mobile(request, command)
            result = value['operation']
            path = value.get('connection_path') or 'unknown'
            self.paths[path] += 1
            with self.trace.open('a', encoding='utf-8') as output:
                output.write(json.dumps({'request_id': request, 'action': action,
                    'state': result['state'], 'error': result.get('error'),
                    'action_dispatched': result['action_dispatched'], 'connection_path':path}) + '\n')
            self.calls += 1
            if result['state'] == 'completed':
                self.last_input_request = request
                self.last_input_command = command
                time.sleep(0.2)
                return result
            if result['state'] in ('running', 'unconfirmed') or result['action_dispatched']:
                raise UnconfirmedOutcome(f'Query original input before continuing: {request}')
            if result['state'] != 'failed' or result.get('error') != 'target_stale' or result['action_dispatched']:
                raise RuntimeError(f'Input outcome must be queried, never replayed: {request} {result["state"]} {result.get("error")}')
        raise RuntimeError('Test control kept changing before dispatch')

    def wait(self, text, *, present=True):
        result, _ = self.call({'action': 'wait_for', 'text': text, 'present': present,
            'timeout_ms': 5000, 'interval_ms': 200})
        assert self.has(result['observation'], text) == present
        return result['observation']

    def fixture(self):
        screen = self.query()
        if screen.get('package_name') != self.package or not (
                self.named(screen, 'fixture_input') or self.named(screen, 'fixture_second_input')):
            self.call({'action': 'launch_app', 'package_name': self.package})
            time.sleep(0.5)
            screen = self.query()
            if not (self.named(screen, 'fixture_input') or self.named(screen, 'fixture_second_input')):
                entry = ('打开验收页面', 'Open acceptance screen')
                # Product UI keeps diagnostics under Settings > About. Navigate
                # using real MCP input and fresh references, never an ADB launch.
                route = (('设置', 'Settings'), ('关于 Pixels Agent Bridge', 'About Pixels Agent Bridge', '关于 Pixels', 'About Pixels'), entry)
                if any(self.named(screen, label) for label in entry):
                    route = (entry,)  # Earlier APKs had a direct home entry.
                for labels in route:
                    for _ in range(8):
                        screen = self.query()
                        assert screen.get('package_name') == self.package
                        name = next((label for label in labels if self.named(screen, label)), None)
                        if name:
                            self.act(name)
                            break
                        containers = self.query(role='android.widget.ScrollView')['elements']
                        if len(containers) != 1:
                            raise ControlMissingBeforeDispatch('The own page has no unique scroll container')
                        outcome, _ = self.call({'action':'scroll', 'element_ref':containers[0]['element_ref'],
                            'direction':'forward'}, allow_pre_dispatch_errors=('target_stale','unsupported_action'))
                        if outcome.get('error') == 'unsupported_action':
                            raise ControlMissingBeforeDispatch('Own diagnostic entry is absent at the scroll boundary')
                        time.sleep(.4)
                    else:
                        raise ControlMissingBeforeDispatch('The own diagnostic entry is not reachable')
        screen = self.query()
        if self.named(screen, 'fixture_second_input'):
            self.act('fixture_previous_page')
        return self.wait('fixture_input')

    def count(self):
        screen = self.query(text='fixture_save_count:')
        name = next(node['name'] for node in screen['elements'] if (node.get('name') or '').startswith('fixture_save_count:'))
        return int(name.split(':')[1])

    def scroll(self, direction):
        self.act('fixture_scroll_list', 'scroll', direction=direction)

    def top(self):
        for _ in range(12):
            if self.named(self.query(text='fixture_item_1'), 'fixture_item_1'):
                return
            self.scroll('backward')
        raise RuntimeError('The fixture list did not return to its first item')

    def basics(self, rounds):
        result, _ = self.call({'action': 'capabilities'})
        capabilities = result['capabilities']
        required = {'observe','query','click','long_click','set_text','scroll','gesture','navigate','launch_app','wait_for','screenshot'}
        assert capabilities['schema_version'] == 2 and required <= set(capabilities['actions'])
        timings = []
        for number in range(rounds):
            started = time.monotonic()
            self.fixture()
            before = self.count()
            text = f'真机远程输入-{number}'
            self.act('fixture_input', 'set_text', text=text)
            self.act('fixture_save')
            self.wait(text)
            assert self.count() == before + 1
            self.act('fixture_long_click', 'long_click')
            self.wait('fixture_long_clicked')
            self.top()
            for attempt in range(3):
                container = self.named(self.query(text='fixture_scroll_list'), 'fixture_scroll_list')
                result, _ = self.call({'action':'query','root_ref':container['element_ref'],
                    'text_contains':'fixture_item_1','role':None,'max_nodes':128,'max_depth':32},
                    allow_pre_dispatch_errors=('target_stale',))
                if result['state'] == 'completed':
                    subtree = result['observation']
                    break
                assert result.get('error') == 'target_stale' and not result['action_dispatched']
                time.sleep(.2)  # Reacquire only an expired read reference; no input is replayed.
            else:
                raise RuntimeError('Fixture subtree kept changing before its read')
            assert self.named(subtree, 'fixture_item_1')
            self.scroll('forward')
            assert not self.named(self.query(text='fixture_item_1'), 'fixture_item_1')
            self.scroll('backward')
            assert self.named(self.query(text='fixture_item_1'), 'fixture_item_1')
            # The gesture must produce a visible effect, not just an Android callback.
            for attempt in range(3):
                screen = self.query()
                button = self.named(screen, 'fixture_save')
                bounds = button['bounds']
                x, y = (bounds['left'] + bounds['right']) // 2, (bounds['top'] + bounds['bottom']) // 2
                outcome, _ = self.call({'action':'gesture','observation_ref':screen['observation_ref'],
                    'from_x':x,'from_y':y,'to_x':x,'to_y':y,'duration_ms':80},
                    allow_pre_dispatch_errors=('target_stale',))
                if outcome['state'] == 'completed':
                    break
                # A fling can invalidate geometry after observation. Only a
                # confirmed pre-dispatch rejection permits fresh observation.
                assert outcome.get('error') == 'target_stale' and not outcome['action_dispatched']
                time.sleep(.2)
            else:
                raise RuntimeError('Fixture geometry did not settle; no gesture was replayed')
            self.wait(f'fixture_save_count:{before + 2}')
            assert self.count() == before + 2
            image, raw = self.call({'action':'screenshot'})
            payload = next(item for item in raw['content'] if item['type'] == 'image')
            jpeg = base64.b64decode(payload['data'], validate=True)
            from PIL import Image
            with Image.open(io.BytesIO(jpeg)) as decoded:
                assert decoded.size == (image['image']['width'], image['image']['height'])
                decoded.verify()
            self.call({'action':'wait_for','text':'fixture_never_present','present':True,
                'timeout_ms':200,'interval_ms':100}, expected_error='condition_timeout')
            self.call({'action':'navigate','destination':'home'})
            time.sleep(0.3)
            assert self.query().get('package_name') != self.package
            self.call({'action':'navigate','destination':'back'})
            self.fixture()
            timings.append(round((time.monotonic() - started) * 1000))
            print(f'Android primitives {number + 1}/{rounds} passed', flush=True)
        return {'rounds':rounds, 'capabilities':capabilities, 'round_elapsed_ms':timings,
                'jpeg_decoded':True, 'gesture_postcondition':True, 'bounded_wait_timeout':True,
                'observed_connection_paths':dict(self.paths)}

    def scenarios(self, rounds):
        results = {name: {'requested':rounds,'attempted':0,'passed':0,'failures':[]} for name in ('multi_page_form','scroll_find','application_switch')}
        for name, report in results.items():
            for number in range(rounds):
                report['attempted'] += 1
                stage = 'open_fixture'
                try:
                    self.fixture()
                    if name == 'multi_page_form':
                        stage = 'fill_form'
                        before = self.count()
                        first, second = f'表单姓名-{number}', f'表单备注-{number}'
                        self.act('fixture_input', 'set_text', text=first)
                        self.act('fixture_next_page')
                        self.wait('fixture_second_input')
                        self.act('fixture_second_input', 'set_text', text=second)
                        self.act('fixture_submit_form')
                        self.wait(f'fixture_form_saved:{first}:{second}')
                        stage = 'verify_single_submit'
                        assert self.count() == before + 1
                        self.act('fixture_previous_page')
                    elif name == 'scroll_find':
                        stage = 'find_and_select'
                        self.top()
                        for _ in range(12):
                            if self.named(self.query(text='fixture_item_35'), 'fixture_item_35'):
                                break
                            self.scroll('forward')
                        else:
                            raise RuntimeError('Target list item was not found within the scroll budget')
                        self.act('fixture_item_35')
                        self.wait('fixture_selected:35')
                    else:
                        stage = 'switch_apps'
                        self.call({'action':'launch_app','package_name':'com.android.settings'})
                        time.sleep(0.5)
                        assert self.query().get('package_name') == 'com.android.settings'
                        self.call({'action':'launch_app','package_name':self.package})
                        time.sleep(0.5)
                        assert self.query().get('package_name') == self.package
                        self.fixture()
                    report['passed'] += 1
                except UnconfirmedOutcome:
                    raise
                except (AssertionError, RuntimeError, StopIteration) as error:
                    report['failures'].append({'round':number + 1,'kind':type(error).__name__, 'stage':stage})
                    # Do not replay the failed task. A subsequent round has independent test data.
                finally:
                    self.trace.with_name('scenarios.json').write_text(json.dumps(results, indent=2), encoding='utf-8')
                print(f'Android scenario {name} {number + 1}/{rounds}: {report["passed"]} passed', flush=True)
        return results
