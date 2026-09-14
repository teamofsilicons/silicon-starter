#!/usr/bin/env bash
# Build on macOS with Xcode command-line tools, Rust, cargo-zigbuild, and Zig.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $(uname -s) == Darwin ]] || { echo 'Build the four release targets on macOS.' >&2; exit 1; }
rustup target add aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-musl x86_64-unknown-linux-musl
MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --release --locked -p starter \
  --target aarch64-apple-darwin --target x86_64-apple-darwin
cargo zigbuild --release --locked -p starter \
  --target aarch64-unknown-linux-musl --target x86_64-unknown-linux-musl
output=target/cli-release
mkdir -p "$output"
for target in aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-musl x86_64-unknown-linux-musl; do
  case "$target" in *-apple-darwin) codesign --force --sign - "target/$target/release/starter" ;; esac
  COPYFILE_DISABLE=1 tar -czf "$output/starter-$target.tar.gz" -C "target/$target/release" starter
done
cp install.sh "$output/install.sh"
(cd "$output" && shasum -a 256 starter-*.tar.gz install.sh > SHA256SUMS)
printf 'Release assets: %s\n' "$output"
