#!/usr/bin/env python3
"""Package six prebuilt native CLIs for Silicon Apps; requires Python 3.11+."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib

root = Path(__file__).resolve().parent.parent
version = tomllib.loads((root / "crates/cli/Cargo.toml").read_text())["package"]["version"]
apps = shutil.which("silicon-apps") or str(
    Path(os.environ.get("SILICON_HOME", Path.home())) / ".apps/bin/silicon-apps"
)
targets = {
    "macos-aarch64": "aarch64-apple-darwin",
    "macos-x86_64": "x86_64-apple-darwin",
    "linux-aarch64": "aarch64-unknown-linux-musl",
    "linux-x86_64": "x86_64-unknown-linux-musl",
    "windows-aarch64": "aarch64-pc-windows-msvc",
    "windows-x86_64": "x86_64-pc-windows-msvc",
}
output = root / "target/cli-release"
output.mkdir(parents=True, exist_ok=True)
for platform, triple in targets.items():
    name = "starter.exe" if platform.startswith("windows-") else "starter"
    binary = root / "target" / triple / "release" / name
    if not binary.is_file() or binary.stat().st_size == 0:
        raise SystemExit(f"Missing native build: {binary}; run scripts/build-cli-release.sh")
    with tempfile.TemporaryDirectory(prefix="starter-package-", dir=output) as temporary:
        package = Path(temporary)
        (package / "bin").mkdir()
        shutil.copyfile(binary, package / "bin" / name)
        (package / "bin" / name).chmod(0o755)
        (package / "apps.yaml").write_text(
            f"schema_version: 1\napp_id: starter\nversion: {version}\ncommand: starter\n"
            f"targets:\n  {platform}:\n    binary: bin/{name}\n"
        )
        archive = output / f"starter-apps-{version}-{platform}.tar.gz"
        subprocess.run([apps, "pack", str(package), "--output", str(archive)], check=True)
        print(archive)
