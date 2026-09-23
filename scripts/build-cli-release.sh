#!/usr/bin/env bash
# macOS: Xcode tools, Rust, cargo-zigbuild, Zig, cargo-xwin, LLVM/lld, Python 3.11+, Honeycomb.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $(uname -s) == Darwin ]] || { echo 'Build the six release targets on macOS.' >&2; exit 1; }
rustup target add aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-musl x86_64-unknown-linux-musl aarch64-pc-windows-msvc x86_64-pc-windows-msvc
MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --release --locked -p starter \
  --target aarch64-apple-darwin --target x86_64-apple-darwin
cargo zigbuild --release --locked -p starter \
  --target aarch64-unknown-linux-musl --target x86_64-unknown-linux-musl
RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static" cargo xwin build --cross-compiler clang --release --locked -p starter \
  --target aarch64-pc-windows-msvc --target x86_64-pc-windows-msvc
output=target/cli-release
mkdir -p "$output"
for target in aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-musl x86_64-unknown-linux-musl; do
  case "$target" in *-apple-darwin) codesign --force --sign - "target/$target/release/starter" ;; esac
  COPYFILE_DISABLE=1 tar -czf "$output/starter-$target.tar.gz" -C "target/$target/release" starter
done
python3 scripts/package-honeycomb.py
cp install.sh "$output/install.sh"
version=$(python3 -c 'import tomllib; print(tomllib.load(open("crates/cli/Cargo.toml", "rb"))["package"]["version"])')
(cd "$output" && shasum -a 256 starter-{aarch64,x86_64}-{apple-darwin,unknown-linux-musl}.tar.gz "starter-honeycomb-$version.tar.gz" install.sh > SHA256SUMS)
printf 'Release assets: %s\n' "$output"
