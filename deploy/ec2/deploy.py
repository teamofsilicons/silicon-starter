#!/usr/bin/env python3
"""Manually deploy a compiled native ARM64 release through S3 and SSM."""
import argparse, hashlib, json, pathlib, shlex, subprocess, tarfile, tempfile, time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', type=pathlib.Path, default=pathlib.Path('target/aarch64-unknown-linux-musl/release/silicon-starter-api'))
parser.add_argument('--region', default='us-east-1')
args = parser.parse_args()

def aws(*parts):
    return subprocess.check_output(['aws', '--region', args.region, *parts], text=True)

binary = args.binary.read_bytes()
if binary[:4] != b'\x7fELF' or binary[18:20] != b'\xb7\x00':
    raise SystemExit('Expected a Linux ARM64 ELF binary')
stack = json.loads(aws('cloudformation', 'describe-stacks', '--stack-name', 'silicon-starter-native'))['Stacks'][0]
outputs = {x['OutputKey']: x['OutputValue'] for x in stack['Outputs']}
base = pathlib.Path(__file__).resolve().parent
with tempfile.TemporaryDirectory() as tmp:
    archive = pathlib.Path(tmp) / 'release.tar.gz'
    with tarfile.open(archive, 'w:gz') as tar:
        tar.add(args.binary, arcname='starter-api')
        for name in ['install.sh', 'starter-api.service', 'refresh-database.py', 'starter-db-refresh.service', 'starter-db-refresh.timer']:
            tar.add(base / name, arcname=name)
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    release = checksum[:16]
    uri = f"s3://{outputs['ArtifactBucket']}/releases/{release}.tar.gz"
    aws('s3', 'cp', str(archive), uri, '--sse', 'AES256', '--only-show-errors')
    remote = '/opt/starter/releases/' + release
    command = '\n'.join(['set -e', 'umask 022', f'mkdir -p {remote}',
        f"aws s3 cp {shlex.quote(uri)} {remote}.tgz --region {args.region} --only-show-errors",
        f"echo '{checksum}  {remote}.tgz' | sha256sum -c -",
        f'tar -xzf {remote}.tgz -C {remote}', f'chmod 755 {remote}/starter-api',
        f'bash {remote}/install.sh {remote} {args.region}'])
    request = pathlib.Path(tmp) / 'request.json'
    request.write_text(json.dumps({'DocumentName': 'AWS-RunShellScript', 'InstanceIds': [outputs['InstanceId']], 'Parameters': {'commands': [command]}, 'Comment': 'Native Starter release ' + release}))
    cid = json.loads(aws('ssm', 'send-command', '--cli-input-json', 'file://' + str(request)))['Command']['CommandId']
print(json.dumps({'release': release, 'instance': outputs['InstanceId'], 'command_id': cid}), flush=True)
for _ in range(90):
    time.sleep(2)
    result = json.loads(aws('ssm', 'get-command-invocation', '--command-id', cid, '--instance-id', outputs['InstanceId']))
    if result['Status'] in ['Pending', 'InProgress', 'Delayed']:
        continue
    print(result['Status'], result['StandardOutputContent'], result['StandardErrorContent'])
    raise SystemExit(0 if result['Status'] == 'Success' else 1)
raise SystemExit('Deployment still running; inspect the command ID before retrying.')
