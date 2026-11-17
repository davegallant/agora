#!/bin/sh
# Install agora from its GitHub release, for macOS or Linux (arm64 or x86_64).
#
#   curl -fsSL https://raw.githubusercontent.com/davegallant/agora/main/install.sh | sh
#
# AGORA_VERSION picks a release (e.g. v0.2.0; default: the latest) and
# AGORA_INSTALL_DIR where the binary goes (default: ~/.local/bin).
set -eu

repo=davegallant/agora
dir=${AGORA_INSTALL_DIR:-$HOME/.local/bin}

fail() {
	echo "agora install: $*" >&2
	exit 1
}

case "$(uname -s)" in
Darwin) os=apple-darwin ;;
Linux) os=unknown-linux-gnu ;;
*) fail "no prebuilt binary for $(uname -s); try: cargo install --git https://github.com/$repo" ;;
esac
case "$(uname -m)" in
arm64 | aarch64) arch=aarch64 ;;
x86_64 | amd64) arch=x86_64 ;;
*) fail "no prebuilt binary for $(uname -m); try: cargo install --git https://github.com/$repo" ;;
esac
target=$arch-$os

version=${AGORA_VERSION:-}
if [ -z "$version" ]; then
	# The latest-release page redirects to its tag; no API token needed.
	latest=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest")
	version=${latest##*/}
	case "$version" in v*) ;; *) fail "could not find the latest release" ;; esac
fi

name=agora-$version-$target
url=https://github.com/$repo/releases/download/$version/$name.tar.gz
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "downloading agora $version for $target"
curl -fsSL -o "$tmp/$name.tar.gz" "$url" || fail "could not download $url"
curl -fsSL -o "$tmp/$name.tar.gz.sha256" "$url.sha256" || fail "could not download $url.sha256"

if command -v sha256sum >/dev/null 2>&1; then
	(cd "$tmp" && sha256sum -c "$name.tar.gz.sha256" >/dev/null)
else
	(cd "$tmp" && shasum -a 256 -c "$name.tar.gz.sha256" >/dev/null)
fi || fail "checksum mismatch for $name.tar.gz"

tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
mkdir -p "$dir"
install -m 755 "$tmp/$name/agora" "$dir/agora"
echo "installed $dir/agora"

case ":$PATH:" in
*":$dir:"*) ;;
*) echo "note: $dir is not on your PATH; add it to run agora" ;;
esac
