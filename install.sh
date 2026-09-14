#!/bin/sh
set -eu

fail() { printf 'starter installer: %s\n' "$*" >&2; exit 1; }
for tool in curl tar awk mktemp sed grep; do
    command -v "$tool" >/dev/null 2>&1 || fail "$tool is required"
done
if command -v sha256sum >/dev/null 2>&1; then
    checksum=sha256sum
elif command -v shasum >/dev/null 2>&1; then
    checksum=shasum
else
    fail 'sha256sum or shasum is required to verify the download'
fi

case "$(uname -s)" in
    Darwin) platform=apple-darwin ;;
    Linux) platform=unknown-linux-musl ;;
    *) fail 'supported systems are macOS and Linux' ;;
esac
case "$(uname -m)" in
    arm64|aarch64) arch=aarch64 ;;
    x86_64|amd64) arch=x86_64 ;;
    *) fail 'supported architectures are ARM64 and x86_64' ;;
esac
repository=https://github.com/teamofsilicons/silicon-starter
version=${STARTER_VERSION:-}
if [ -z "$version" ]; then
    latest=$(curl -fLsS -o /dev/null -w '%{url_effective}' "$repository/releases/latest") || fail 'cannot resolve the latest release'
    case "$latest" in
        "$repository/releases/tag/"*) version=${latest#"$repository/releases/tag/"} ;;
        *) fail 'GitHub did not return a release tag' ;;
    esac
fi
case "$version" in
    ''|.|..|*[!a-zA-Z0-9._-]*) fail 'invalid release tag; use a tag such as v0.1.0' ;;
esac
asset=starter-$arch-$platform.tar.gz
url=$repository/releases/download/$version
work=$(mktemp -d)
stage=
use_sudo=0
run_install() {
    if [ "$use_sudo" = 1 ]; then sudo "$@"; else "$@"; fi
}
cleanup() {
    [ -z "$stage" ] || run_install rm -f "$stage" || true
    rm -rf "$work"
}
trap cleanup 0
trap 'exit 1' HUP INT TERM
curl -fLsS "$url/SHA256SUMS" -o "$work/SHA256SUMS" || fail "cannot download checksums for $version"
curl -fLsS "$url/$asset" -o "$work/$asset" || fail "cannot download $asset for $version"
expected=$(awk -v asset="$asset" '$2 == asset || $2 == "*" asset {print $1}' "$work/SHA256SUMS")
case "$expected" in ''|*[!a-f0-9]*) fail "missing or invalid checksum for $asset" ;; esac
[ "${#expected}" = 64 ] || fail "missing or duplicate checksum for $asset"
if [ "$checksum" = sha256sum ]; then
    actual=$(sha256sum < "$work/$asset")
else
    actual=$(shasum -a 256 < "$work/$asset")
fi
[ "${actual%% *}" = "$expected" ] || fail 'checksum mismatch; nothing was installed'
[ "$(tar -tzf "$work/$asset")" = starter ] || fail 'release archive must contain only starter'
tar -xOzf "$work/$asset" starter > "$work/starter" || fail 'cannot extract starter'
[ -s "$work/starter" ] || fail 'release contains an empty binary'

if [ "${STARTER_INSTALL_DIR+x}" = x ]; then
    install_dir=$STARTER_INSTALL_DIR
elif { [ -d /usr/local/bin ] && [ -w /usr/local/bin ]; } || { [ ! -e /usr/local/bin ] && [ -w /usr/local ]; }; then
    install_dir=/usr/local/bin
elif command -v sudo >/dev/null 2>&1 && ( : </dev/tty ) 2>/dev/null && sudo -v </dev/tty; then
    install_dir=/usr/local/bin
    use_sudo=1
else
    install_dir=${HOME:?HOME is required}/.local/bin
fi
case "$install_dir" in /*) ;; *) fail 'STARTER_INSTALL_DIR must be an absolute path' ;; esac
case "$install_dir" in *:*|*'
'*|*"$(printf '\r')"*) fail 'installation path cannot contain colons or line breaks' ;; esac
[ ! -d "$install_dir/starter" ] || fail "$install_dir/starter is a directory; nothing was replaced"
run_install mkdir -p "$install_dir" || fail "cannot create $install_dir; choose a writable STARTER_INSTALL_DIR"
stage=$(run_install mktemp "$install_dir/.starter.XXXXXXXX") || fail "cannot write to $install_dir"
run_install cp "$work/starter" "$stage"
run_install chmod 755 "$stage"
run_install mv -f "$stage" "$install_dir/starter"
stage=
printf 'Installed Starter %s to %s/starter\n' "$version" "$install_dir"

case ":${PATH:-}:" in
    *:"$install_dir":*) printf 'Run: starter --help\n' ;;
    *)
        shell_name=${SHELL:-sh}
        shell_name=${shell_name##*/}
        escaped=$(printf '%s' "$install_dir" | sed 's/\\/\\\\/g; s/"/\\"/g; s/\$/\\$/g')
        if [ "$shell_name" != fish ]; then
            escaped=$(printf '%s' "$escaped" | sed 's/`/\\`/g')
        fi
        append_path() {
            config=$1
            marker="# <<< silicon-starter PATH: $install_dir"
            if [ -f "$config" ] && grep -Fqx "$marker" "$config"; then return; fi
            mkdir -p "$(dirname "$config")" || fail "installed binary, but cannot create shell configuration at $config"
            if [ "$shell_name" = fish ]; then
                block=$(printf 'if not contains -- "%s" $PATH\n    set -gx PATH "%s" $PATH\nend' "$escaped" "$escaped")
            else
                block=$(printf 'case ":$PATH:" in\n    *:"%s":*) ;;\n    *) export PATH="%s:$PATH" ;;\nesac' "$escaped" "$escaped")
            fi
            printf '\n# >>> silicon-starter PATH\n%s\n%s\n' "$block" "$marker" >> "$config" || fail "installed binary, but cannot add PATH to $config"
            printf 'Added PATH setup to %s\n' "$config"
        }
        case "$shell_name" in
            bash)
                append_path "$HOME/.bashrc"
                if [ -f "$HOME/.bash_profile" ]; then append_path "$HOME/.bash_profile"
                elif [ -f "$HOME/.bash_login" ]; then append_path "$HOME/.bash_login"
                else append_path "$HOME/.profile"; fi
                ;;
            zsh) append_path "${ZDOTDIR:-$HOME}/.zshrc" ;;
            fish) append_path "${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish" ;;
            sh|dash|ksh|'') append_path "$HOME/.profile" ;;
            *) printf 'Add %s to PATH in your shell configuration.\n' "$install_dir" ;;
        esac
        printf 'Open a new terminal (or reload your shell configuration) before running starter.\n'
        ;;
esac
if ! command -v git >/dev/null 2>&1; then
    printf 'Git was not found. Install Git 2.28+ before using starter repository commands.\n' >&2
fi
