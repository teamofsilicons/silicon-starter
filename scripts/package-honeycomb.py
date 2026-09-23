#!/usr/bin/env python3
"""Package the six prebuilt CLIs for Honeycomb; requires Python 3.11+."""
import io
from pathlib import Path
import subprocess
import tarfile
import tomllib

root = Path(__file__).resolve().parent.parent
version = tomllib.loads((root / "crates/cli/Cargo.toml").read_text())["package"]["version"]
targets = {
    "macos-aarch64": "aarch64-apple-darwin",
    "macos-x86_64": "x86_64-apple-darwin",
    "linux-aarch64": "aarch64-unknown-linux-musl",
    "linux-x86_64": "x86_64-unknown-linux-musl",
    "windows-aarch64": "aarch64-pc-windows-msvc",
    "windows-x86_64": "x86_64-pc-windows-msvc",
}
manifest = f"format_version: 1\napp_id: starter\nversion: {version}\nbin:\n  starter: main\ntargets:\n"
payloads = []
for platform, triple in targets.items():
    name = "starter.exe" if platform.startswith("windows-") else "starter"
    binary = root / "target" / triple / "release" / name
    if not binary.is_file() or binary.stat().st_size == 0:
        raise SystemExit(f"Missing native build: {binary}; run scripts/build-cli-release.sh")
    manifest += f"  {platform}:\n    root: targets/{platform}\n    executables:\n      main: {name}\n"
    payloads.append((binary, f"targets/{platform}/{name}"))

output = root / "target/cli-release" / f"starter-honeycomb-{version}.tar.gz"
output.parent.mkdir(parents=True, exist_ok=True)
with tarfile.open(output, "w:gz", format=tarfile.USTAR_FORMAT) as archive:
    header = tarfile.TarInfo("honeycomb.yaml")
    content = manifest.encode()
    header.size = len(content)
    header.mode = 0o644
    archive.addfile(header, io.BytesIO(content))
    for binary, name in payloads:
        header = tarfile.TarInfo(name)
        header.size = binary.stat().st_size
        header.mode = 0o755
        with binary.open("rb") as source:
            archive.addfile(header, source)
subprocess.run(["honeycomb", "validate", str(output)], check=True)
print(output)
