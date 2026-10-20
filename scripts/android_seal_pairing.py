#!/usr/bin/env python3
"""Seal one normal, visibly revealed phone password for an isolated remote operator.

Only the remote host's ephemeral public key leaves that host. The sealed envelope
is runtime transfer data; no plaintext credential is written to the host filesystem.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
from pathlib import Path
import tomllib
import uuid
import xml.etree.ElementTree as ET

from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.asymmetric import padding, rsa

from android_home import Home
from test_android_remote import PACKAGE, adb, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--apk-sha256', required=True)
    parser.add_argument('--run-id', required=True, type=uuid.UUID)
    parser.add_argument('--public-key', required=True, type=Path)
    parser.add_argument('--public-key-sha256', required=True)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    assert installed_apk_sha256(args.serial) == args.apk_sha256
    assert not args.output.exists(), 'Never replace a previously sealed transfer'
    key_bytes = args.public_key.read_bytes()
    assert hashlib.sha256(key_bytes).hexdigest() == args.public_key_sha256.lower()
    xml = ET.fromstring(key_bytes)
    number = lambda name: int.from_bytes(base64.b64decode(xml.findtext(name), validate=True), 'big')
    key = rsa.RSAPublicNumbers(number('Exponent'), number('Modulus')).public_key()
    assert key.key_size >= 2048
    settings = tomllib.loads(adb(args.serial, 'shell', 'run-as', PACKAGE, 'cat', 'no_backup/settings.toml'))
    assert settings['control_url'] == 'wss://pab.rgaa.vip/control' and not settings.get('trusted_ca_pem')
    home = Home(args.serial)
    home.start()
    code, password = home.revealed_credentials()
    payload = json.dumps({'run_id':str(args.run_id), 'device_code':code,
        'password':password}, separators=(',', ':')).encode()
    sealed = key.encrypt(payload, padding.OAEP(mgf=padding.MGF1(hashes.SHA256()),
        algorithm=hashes.SHA256(), label=None))
    del password, payload
    with args.output.open('xb') as stream:
        stream.write(sealed)
    print('One-time pairing envelope sealed; no plaintext password retained')


if __name__ == '__main__':
    main()
