#!/usr/bin/env python3
"""Runnable credential-refresh regression without AWS or real credentials."""
import importlib.util
import json
import pathlib
import tempfile
import sys

sys.dont_write_bytecode = True

source = pathlib.Path(__file__).with_name('refresh-database.py')
spec = importlib.util.spec_from_file_location('refresh_database', source)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
with tempfile.TemporaryDirectory() as folder:
    path = pathlib.Path(folder) / 'api.env'
    original = 'UNCHANGED="yes"\nSTARTER_DATABASE_URL="postgres://starter_admin:old@database:5432/starter?sslmode=require"\n'
    path.write_text(original)
    credential = {'username': 'starter_admin', 'password': 'new@p:/%"\\word'}
    assert module.refresh(path, credential)
    updated = path.read_text()
    assert updated.startswith('UNCHANGED="yes"\n')
    url = module.urllib.parse.urlsplit(json.loads(updated.split('STARTER_DATABASE_URL=')[1]))
    assert module.urllib.parse.unquote(url.password) == credential['password']
    assert url.hostname == 'database' and url.port == 5432
    assert url.path == '/starter' and url.query == 'sslmode=require'
    assert path.stat().st_mode & 0o777 == 0o600
    assert not module.refresh(path, credential)
    try:
        module.refresh(path, {'username': 'other', 'password': 'other'})
    except ValueError:
        pass
    else:
        raise AssertionError('Mismatched username was accepted')
    assert path.read_text() == updated
print('Database refresh checks passed.')
