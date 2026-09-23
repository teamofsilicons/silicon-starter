#!/usr/bin/env python3
"""Refresh only Starter's rotated RDS credential; never print secret values."""
import argparse
import fcntl
import json
import os
import pathlib
import subprocess
import tempfile
import urllib.parse

MANAGED_SECRET = 'arn:aws:secretsmanager:us-east-1:234951665042:secret:rds!db-ce5cfadb-b038-43fc-a7c3-a6c0663cf3ad-UxnQ83'
ENVIRONMENT = pathlib.Path('/etc/starter/starter-api.env')


def refresh(path, credential):
    original = path.read_text()
    lines = original.splitlines(keepends=True)
    matches = [i for i, line in enumerate(lines) if line.startswith('STARTER_DATABASE_URL=')]
    if len(matches) != 1:
        raise ValueError('Expected exactly one configured Starter database URL')
    index = matches[0]
    current = json.loads(lines[index].split('=', 1)[1])
    url = urllib.parse.urlsplit(current)
    if urllib.parse.unquote(url.username or '') != credential['username']:
        raise ValueError('Managed database username does not match Starter configuration')
    username = urllib.parse.quote(credential['username'], safe='')
    password = urllib.parse.quote(credential['password'], safe='')
    updated = urllib.parse.urlunsplit(url._replace(netloc=username + ':' + password + '@' + url.netloc.rsplit('@', 1)[1]))
    if updated == current:
        return False
    lines[index] = 'STARTER_DATABASE_URL=' + json.dumps(updated) + '\n'
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode='w', dir=path.parent, delete=False) as output:
            temporary = pathlib.Path(output.name)
            output.write(''.join(lines))
        os.replace(temporary, path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--region', default='us-east-1')
    parser.add_argument('--no-restart', action='store_true', help='Installer already holds the deployment lock and will restart the API')
    args = parser.parse_args()
    lock = None
    if not args.no_restart:
        lock = open('/var/lock/starter-deploy.lock', 'a')
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return
    result = subprocess.check_output(['aws', 'secretsmanager', 'get-secret-value', '--region', args.region, '--secret-id', MANAGED_SECRET], text=True)
    credential = json.loads(json.loads(result)['SecretString'])
    if refresh(ENVIRONMENT, credential):
        print('Refreshed Starter database credential.')
        if not args.no_restart:
            subprocess.run(['systemctl', 'restart', 'starter-api'], check=True)


if __name__ == '__main__':
    main()
