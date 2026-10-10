"""Create the offline Ed25519 release key and print only its public half."""

from argparse import ArgumentParser
from base64 import b64encode
from pathlib import Path
import os

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey


def main():
    parser = ArgumentParser(description=__doc__)
    parser.add_argument('private_key', type=Path)
    args = parser.parse_args()
    path = args.private_key.expanduser().resolve()
    path.parent.mkdir(parents=True, exist_ok=True)
    private = Ed25519PrivateKey.generate()
    raw = private.private_bytes(serialization.Encoding.Raw, serialization.PrivateFormat.Raw, serialization.NoEncryption())
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    fd = os.open(path, flags, 0o600)
    try:
        with os.fdopen(fd, 'wb') as file:
            file.write(raw)
    except BaseException:
        path.unlink(missing_ok=True)
        raise
    public = private.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
    print('Public key (safe to put in site.toml and Desktop): ' + b64encode(public).decode())
    print('Private key saved locally. Back it up securely; do not commit it.')


if __name__ == '__main__':
    main()
