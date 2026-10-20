#!/usr/bin/env python3
"""Verify the real public account API rejects a revoked native session token.

The temporary account password and token exist in memory only. This is account
policy evidence, not a claim that an already dispatched mobile input was undone.
"""
import json
from pathlib import Path
import time
import urllib.error
import urllib.request
import uuid


def main():
    run = uuid.uuid4()
    root = Path(__file__).resolve().parents[1] / '.build/android/account-revocation' / str(run)
    root.mkdir(parents=True)
    url = 'https://pab.rgaa.vip/api/account'
    token = None
    statuses = []
    result = {'status':'failed', 'run_id':str(run), 'started_at_unix':time.time(),
        'public_server':True, 'scope':'native_account_session_revocation'}

    def request(path, method='GET', payload=None):
        headers = {'Content-Type':'application/json'}
        if token is not None:
            headers['Authorization'] = 'Bearer ' + token
        message = urllib.request.Request(url + path, method=method, headers=headers,
            data=json.dumps(payload).encode() if payload is not None else None)
        try:
            with urllib.request.urlopen(message, timeout=30) as response:
                status, body = response.status, response.read()
        except urllib.error.HTTPError as error:
            status, body = error.code, error.read()
        statuses.append({'path':path, 'method':method, 'status':status})
        return status, json.loads(body) if body else None

    try:
        status, value = request('/register', 'POST', {'username':'android-revoke-' + run.hex[:16],
            'password':uuid.uuid4().hex + uuid.uuid4().hex})
        assert status == 200
        token = value['access_token']
        del value
        assert request('/session')[0] == 200
        assert request('/logout', 'POST')[0] == 204
        assert request('/session')[0] == 401
        assert request('/saved-devices')[0] == 401
        result['status'] = 'passed'
        result['revoked_token_rejected'] = True
    finally:
        if token:
            request('/logout', 'POST')
        token = None
        result['requests'] = statuses
        (root / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
        print(json.dumps(result))


if __name__ == '__main__':
    main()
