#!/usr/bin/env python3
"""Package nine prebuilt native CLIs for Silicon Apps; requires Python 3.11+."""
import gzip
import io
from pathlib import Path
import tarfile
import tomllib

root = Path(__file__).resolve().parent.parent
version = tomllib.loads((root / "crates/cli/Cargo.toml").read_text())["package"]["version"]
targets = {
    "macos-aarch64": "aarch64-apple-darwin",
    "macos-x86_64": "x86_64-apple-darwin",
    "linux-aarch64": "aarch64-unknown-linux-musl",
    "linux-x86_64": "x86_64-unknown-linux-musl",
    "linux-i686": "i686-unknown-linux-musl",
    "linux-armv7hf": "armv7-unknown-linux-musleabihf",
    "windows-aarch64": "aarch64-pc-windows-msvc",
    "windows-x86_64": "x86_64-pc-windows-msvc",
    "windows-i686": "i686-pc-windows-msvc",
}
output = root / "target/cli-release"
output.mkdir(parents=True, exist_ok=True)
for platform, triple in targets.items():
    name = "starter.exe" if platform.startswith("windows-") else "starter"
    binary = root / "target" / triple / "release" / name
    if not binary.is_file() or binary.stat().st_size == 0:
        raise SystemExit(f"Missing native build: {binary}; run scripts/build-cli-release.sh")
    manifest = (
        f"schema_version: 1\napp_id: starter\nversion: {version}\ncommand: starter\n"
        f"targets:\n  {platform}:\n    binary: bin/{name}\n"
    ).encode()
    archive = output / f"starter-apps-{version}-{platform}.tar.gz"
    # Fixed archive metadata makes local packaging reproducible and needs no Accounts session.
    with archive.open("wb") as raw, gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.GNU_FORMAT) as package:
            for path, data, mode in [("apps.yaml", manifest, 0o644), (f"bin/{name}", binary.read_bytes(), 0o755)]:
                member = tarfile.TarInfo(path)
                member.size = len(data)
                member.mode = mode
                package.addfile(member, io.BytesIO(data))
    print(archive)
