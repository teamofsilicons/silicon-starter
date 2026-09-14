#!/usr/bin/env python3
"""Exercise install.sh offline in temporary homes; never touch the real PATH or sudo."""
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile

SOURCE = (Path(__file__).resolve().parents[1] / 'install.sh').read_text()
REPOSITORY = 'https://github.com/teamofsilicons/silicon-starter'
BINARY = b'#!/bin/sh\necho installer-must-not-run-this > "$HOME/executed"\n'


def executable(path, text):
    path.write_text(f'#!{sys.executable}\n' + text)
    path.chmod(0o755)


with tempfile.TemporaryDirectory(prefix='starter-installer-check-') as temporary:
    root = Path(temporary)
    tools = root / 'tools'
    tools.mkdir()
    for name in ['tar', 'awk', 'mktemp', 'sed', 'grep', 'dirname', 'mkdir', 'rm', 'cp', 'chmod', 'mv']:
        tools.joinpath(name).symlink_to(shutil.which(name) or sys.exit(f'{name} is required for the check'))
    executable(tools / 'uname', "import os,sys; print(os.environ['CHECK_OS' if sys.argv[1] == '-s' else 'CHECK_ARCH'])\n")
    executable(tools / 'curl', """import os,pathlib,sys
url = next(arg for arg in sys.argv[1:] if arg.startswith('https://'))
root = pathlib.Path(os.environ['CHECK_ROOT'])
with (root / 'requests').open('a') as log: log.write(url + '\\n')
if url.endswith('/releases/latest'):
    print(url.removesuffix('/latest') + '/tag/v0.1.0', end='')
else:
    assert '/releases/download/v0.1.0/' in url, url
    pathlib.Path(sys.argv[sys.argv.index('-o') + 1]).write_bytes((root / url.rsplit('/', 1)[-1]).read_bytes())
""")
    digest_script = "import hashlib,sys; print(hashlib.sha256(sys.stdin.buffer.read()).hexdigest() + '  -')\n"
    executable(tools / 'sha256sum', digest_script)

    def release(target, member='starter', corrupt=False):
        asset = f'starter-{target}.tar.gz'
        with tarfile.open(root / asset, 'w:gz') as archive:
            entry = tarfile.TarInfo(member)
            entry.size = len(BINARY)
            entry.mode = 0o755
            archive.addfile(entry, io.BytesIO(BINARY))
        digest = '0' * 64 if corrupt else hashlib.sha256((root / asset).read_bytes()).hexdigest()
        (root / 'SHA256SUMS').write_text(f'{digest}  {asset}\n')

    def run(name, system='Linux', arch='x86_64', shell='bash', **extra):
        home = root / name
        home.mkdir(exist_ok=True)
        # Remap only the fixed system directory so default selection is also safe to exercise.
        script = home / 'install.sh'
        script.write_text(SOURCE.replace('/usr/local', str(home / 'system')))
        env = {
            'PATH': str(tools), 'HOME': str(home), 'SHELL': f'/bin/{shell}',
            'CHECK_ROOT': str(root), 'CHECK_OS': system, 'CHECK_ARCH': arch,
            **extra,
        }
        result = subprocess.run(['/bin/sh', str(script)], env=env, text=True, capture_output=True)
        return home, result, env

    for system, arch, target in [
        ('Darwin', 'arm64', 'aarch64-apple-darwin'),
        ('Darwin', 'x86_64', 'x86_64-apple-darwin'),
        ('Linux', 'aarch64', 'aarch64-unknown-linux-musl'),
        ('Linux', 'x86_64', 'x86_64-unknown-linux-musl'),
    ]:
        release(target)
        home, result, env = run(f'{system}-{arch}', system, arch)
        assert result.returncode == 0, result.stderr
        destination = home / '.local/bin/starter'
        assert destination.read_bytes() == BINARY and os.access(destination, os.X_OK)
        assert not (home / 'executed').exists(), 'installer executed the downloaded binary'
        assert 'Git 2.28+' in result.stderr and 'Open a new terminal' in result.stdout
        before = (home / '.bashrc').read_text(), (home / '.profile').read_text()
        _, again, _ = run(home.name, system, arch)
        assert again.returncode == 0, again.stderr
        assert before == ((home / '.bashrc').read_text(), (home / '.profile').read_text())
        parsed = subprocess.run(['/bin/sh', '-c', '. "$1"; . "$2"; . "$1"; printf "%s" "$PATH"', 'check', str(home / '.bashrc'), str(home / '.profile')], env=env, text=True, capture_output=True, check=True)
        assert parsed.stdout.split(':').count(str(destination.parent)) == 1

    # Existing bash login selection, literal shell metacharacters, and preservation of user content.
    target = 'x86_64-unknown-linux-musl'
    release(target)
    home = root / 'custom'
    home.mkdir()
    (home / '.bash_login').write_text('# keep existing login configuration\n')
    destination = home / 'bin spaces\'"$MARK`uname`\\literal'
    _, result, env = run('custom', STARTER_INSTALL_DIR=str(destination), STARTER_VERSION='v0.1.0')
    assert result.returncode == 0, result.stderr
    assert (home / '.bash_login').read_text().startswith('# keep existing login configuration\n')
    assert not (home / '.profile').exists()
    parsed = subprocess.run(['/bin/sh', '-c', '. "$1"; printf "%s" "$PATH"', 'check', str(home / '.bash_login')], env=env, text=True, capture_output=True, check=True)
    assert parsed.stdout.split(':')[0] == str(destination)
    old_binary = (destination / 'starter').read_bytes()
    release(target, corrupt=True)
    _, result, _ = run('custom', STARTER_INSTALL_DIR=str(destination))
    assert result.returncode != 0 and 'checksum mismatch' in result.stderr
    assert (destination / 'starter').read_bytes() == old_binary
    release(target, member='../outside')
    _, result, _ = run('custom', STARTER_INSTALL_DIR=str(destination))
    assert result.returncode != 0 and 'must contain only starter' in result.stderr
    assert (destination / 'starter').read_bytes() == old_binary and not (root / 'outside').exists()
    assert not list(destination.glob('.starter.*')), 'partial atomic install was left behind'

    release(target)
    home = root / 'system-path'
    (home / 'system/bin').mkdir(parents=True)
    _, result, _ = run('system-path', PATH=f'{tools}:{home}/system/bin')
    assert result.returncode == 0 and (home / 'system/bin/starter').read_bytes() == BINARY
    assert not (home / '.bashrc').exists(), 'modified PATH when destination was already on PATH'

    # Exercise shasum fallback and the actual startup-file locations for zsh and fish.
    (tools / 'sha256sum').unlink()
    executable(tools / 'shasum', "import sys; assert sys.argv[1:] == ['-a', '256']\n" + digest_script)
    zdot = root / 'zsh-dotdir'
    home, result, env = run('zsh', shell='zsh', ZDOTDIR=str(zdot))
    assert result.returncode == 0 and (zdot / '.zshrc').exists(), result.stderr
    assert not (home / '.zshrc').exists()
    if shutil.which('zsh'):
        subprocess.run([shutil.which('zsh'), '-f', '-n', str(zdot / '.zshrc')], check=True, env=env)
    xdg = root / 'fish-config'
    home, result, env = run('fish', shell='fish', XDG_CONFIG_HOME=str(xdg))
    config = xdg / 'fish/config.fish'
    assert result.returncode == 0 and 'set -gx PATH' in config.read_text(), result.stderr
    before = config.read_text()
    _, result, _ = run('fish', shell='fish', XDG_CONFIG_HOME=str(xdg))
    assert result.returncode == 0 and config.read_text() == before
    if shutil.which('fish'):
        subprocess.run([shutil.which('fish'), '-n', str(config)], check=True, env=env)
    for extra in [dict(system='FreeBSD'), dict(arch='armv7l'), dict(STARTER_VERSION='../../bad')]:
        _, result, _ = run('unsupported', **extra)
        assert result.returncode != 0
    requests = (root / 'requests').read_text().splitlines()
    assert all(url == REPOSITORY + '/releases/latest' or url.startswith(REPOSITORY + '/releases/download/v0.1.0/') for url in requests)

print('Installer checks passed: four targets, pinned assets, checksum/archive rejection, atomic replacement, PATH and startup-file idempotence.')
