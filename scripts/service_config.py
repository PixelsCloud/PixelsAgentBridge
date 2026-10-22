"""Write simple service TOML fixtures; never print configuration contents."""
import json
from pathlib import Path


def write_config(path: Path, values: dict) -> Path:
    def scalar(value):
        if isinstance(value, Path):
            value = str(value)
        if isinstance(value, dict):
            return '{ ' + ', '.join(f'{json.dumps(key)} = {scalar(item)}' for key, item in value.items() if item is not None) + ' }'
        if isinstance(value, (list, tuple)):
            return '[' + ', '.join(scalar(item) for item in value) + ']'
        return json.dumps(value, ensure_ascii=False, allow_nan=False)

    lines = []
    for key, value in values.items():
        if not isinstance(value, dict) and value is not None:
            lines.append(f'{key} = {scalar(value)}')
    for section, fields in values.items():
        if isinstance(fields, dict):
            lines.append(f'\n[{section}]')
            for key, value in fields.items():
                if value is not None:
                    lines.append(f'{key} = {scalar(value)}')
    path.write_text('\n'.join(lines) + '\n', encoding='utf-8')
    path.chmod(0o600)
    return path
