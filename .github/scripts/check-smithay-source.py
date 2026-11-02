#!/usr/bin/env python3
"""Reject bundled, patched, forked, or inconsistently pinned Smithay sources."""
from pathlib import Path
import re
import subprocess
import tomllib

root = Path(__file__).resolve().parents[2]
paths = subprocess.check_output(['git', 'ls-files', '-z'], cwd=root).decode().split('\0')
paths = [Path(p) for p in paths if p and (root / p).is_file()]
for path in paths:
    assert not any(part.lower() in {'vendor', 'vendored', 'smithay'} for part in path.parts), f'Bundled dependency: {path}'
    if path.name == 'Cargo.toml' or path.parts[0] == '.cargo' and path.name in {'config', 'config.toml'}:
        data = tomllib.loads((root / path).read_text())
        assert not any(key in data for key in ('patch', 'replace', 'source')), f'Dependency override: {path}'
        assert not data.get('paths'), f'Path override: {path}'
        # Include target-specific and workspace dependencies, including aliases.
        def check_dependencies(table, location=()):
            for key, value in table.items():
                if key in {'dependencies', 'dev-dependencies', 'build-dependencies'}:
                    for name, spec in value.items():
                        package = spec.get('package', name) if isinstance(spec, dict) else name
                        if name.startswith('smithay') or package.startswith('smithay'):
                            assert path == Path('Cargo.toml') and not location and name in {'smithay', 'smithay-drm-extras'}, f'Unexpected Smithay dependency: {path}:{name}'
                elif isinstance(value, dict):
                    check_dependencies(value, (*location, key))
        check_dependencies(data)

manifest = tomllib.loads((root / 'Cargo.toml').read_text())
revision = None
for name in ('smithay', 'smithay-drm-extras'):
    spec = manifest['dependencies'][name]
    assert spec['git'] == 'https://github.com/Smithay/smithay.git', f'{name}: require official upstream'
    assert re.fullmatch('[0-9a-f]{40}', spec['rev']), f'{name}: require a full revision'
    assert not any(key in spec for key in ('path', 'branch', 'tag')), f'{name}: require only a revision pin'
    assert revision is None or revision == spec['rev'], 'Smithay revisions differ'
    revision = spec['rev']
expected = f'git+https://github.com/Smithay/smithay.git?rev={revision}#{revision}'
lock = tomllib.loads((root / 'Cargo.lock').read_text())
for name in ('smithay', 'smithay-drm-extras'):
    packages = [p for p in lock['package'] if p['name'] == name]
    assert len(packages) == 1 and packages[0].get('source') == expected, f'{name}: lockfile source mismatch'
print(f'Unmodified official Smithay source pinned consistently at {revision}')
