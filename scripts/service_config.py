"""Write simple service TOML fixtures; never print configuration contents."""
import json
from pathlib import Path


def write_config(path: Path, values: dict) -> Path:
    lines = []
    for key, value in values.items():
        if not isinstance(value, dict) and value is not None:
            lines.append(f'{key} = {json.dumps(value, ensure_ascii=False)}')
    for section, fields in values.items():
        if isinstance(fields, dict):
            lines.append(f'\n[{section}]')
            for key, value in fields.items():
                if value is not None:
                    lines.append(f'{key} = {json.dumps(str(value) if isinstance(value, Path) else value, ensure_ascii=False)}')
    path.write_text('\n'.join(lines) + '\n', encoding='utf-8')
    path.chmod(0o600)
    return path
