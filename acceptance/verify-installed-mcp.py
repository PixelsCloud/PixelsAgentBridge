"""Verify an installed stdio server; this does not verify the AI host's reload."""
import argparse
import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import time

parser = argparse.ArgumentParser()
parser.add_argument('binary', type=Path)
parser.add_argument('version')
parser.add_argument('report', type=Path)
args = parser.parse_args()
process = subprocess.Popen([str(args.binary)], stdin=subprocess.PIPE,
    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, encoding='utf-8',
    env={**os.environ, 'PAB_MCP_GUEST': '1'})
messages = queue.Queue()

def read_messages():
    try:
        for line in process.stdout:
            messages.put(json.loads(line))
    except Exception as error:
        messages.put(error)
    finally:
        messages.put(EOFError('MCP stdout closed'))

reader = threading.Thread(target=read_messages, daemon=True)
reader.start()

def send(message):
    process.stdin.write(json.dumps(message) + '\n')
    process.stdin.flush()

def response(request_id):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        message = messages.get(timeout=max(.001, deadline-time.monotonic()))
        if isinstance(message, Exception):
            raise message
        if message.get('id') == request_id:
            assert 'error' not in message, 'MCP returned an error'
            return message['result']
    raise TimeoutError('MCP response deadline')

try:
    send({'jsonrpc':'2.0','id':1,'method':'initialize','params':{
        'protocolVersion':'2025-03-26','capabilities':{},
        'clientInfo':{'name':'installed-acceptance','version':'1'}}})
    initialization = response(1)
    send({'jsonrpc':'2.0','method':'notifications/initialized'})
    send({'jsonrpc':'2.0','id':2,'method':'tools/list','params':{}})
    listing = response(2)
    assert initialization['serverInfo']['version'] == args.version, 'Version mismatch'
    names = {tool['name'] for tool in listing['tools']}
    assert len(listing['tools']) == len(names) == 64, 'Tool directory mismatch'
    assert {'pab_ui_query','pab_ui_get','pab_ui_action','pab_ui_wait'} <= names
    process.stdin.close()
    process.wait(timeout=15)
    assert process.returncode == 0, 'MCP failed to exit cleanly'
    report = {'installed_stdio':initialization['serverInfo'], 'tool_count':len(names),
              'ui_tools_present':True, 'exit_code':process.returncode,
              'current_ai_host_reload':'not_verified'}
    args.report.write_text(json.dumps(report, indent=2)+'\n', encoding='utf-8')
    print(json.dumps(report))
finally:
    if process.poll() is None:
        process.kill()
        process.wait(timeout=10)
    reader.join(timeout=1)
