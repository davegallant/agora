#!/bin/sh
# Install agora from its GitHub release, for macOS or Linux (arm64 or x86_64).
#
#   curl -fsSL https://raw.githubusercontent.com/davegallant/agora/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/davegallant/agora/main/install.sh | sh -s -- --version v0.2.0 --dir /usr/local/bin
#
# Never prompts and needs no sudo for the default directory. Progress goes to
# stderr; on success the only stdout line is the installed binary's absolute
# path. Exit status: 0 installed, 1 failed (the reason is on stderr), 2 bad
# usage.
set -eu

repo=davegallant/agora
version=${AGORA_VERSION:-}
dir=${AGORA_INSTALL_DIR:-$HOME/.local/bin}

usage() {
	cat <<EOF
usage: install.sh [--version vX.Y.Z] [--dir DIR]

  --version  release to install (default: the latest; env AGORA_VERSION)
  --dir      where to put the binary (default: ~/.local/bin; env AGORA_INSTALL_DIR)
EOF
}

say() { echo "agora install: $*" >&2; }
fail() {
	say "$*"
	exit 1
}

while [ $# -gt 0 ]; do
	case "$1" in
	--version)
		[ $# -ge 2 ] || { usage >&2; exit 2; }
		version=$2
		shift 2
		;;
	--dir)
		[ $# -ge 2 ] || { usage >&2; exit 2; }
		dir=$2
		shift 2
		;;
	-h | --help)
		usage
		exit 0
		;;
	*)
		say "unknown argument: $1"
		usage >&2
		exit 2
		;;
	esac
done
case "$version" in "" | v*) ;; *) version=v$version ;; esac

case "$(uname -s)" in
Darwin) os=apple-darwin ;;
Linux) os=unknown-linux-gnu ;;
*) fail "no prebuilt binary for $(uname -s); build it with: cargo install --git https://github.com/$repo" ;;
esac
case "$(uname -m)" in
arm64 | aarch64) arch=aarch64 ;;
x86_64 | amd64) arch=x86_64 ;;
*) fail "no prebuilt binary for $(uname -m); build it with: cargo install --git https://github.com/$repo" ;;
esac
target=$arch-$os

if [ -z "$version" ]; then
	# The latest-release page redirects to its tag; no API token needed.
	latest=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest") ||
		fail "could not reach github.com to find the latest release"
	version=${latest##*/}
	case "$version" in v*) ;; *) fail "could not find the latest release" ;; esac
fi

name=agora-$version-$target
url=https://github.com/$repo/releases/download/$version/$name.tar.gz
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

say "downloading $version for $target"
curl -fsSL -o "$tmp/$name.tar.gz" "$url" 2>/dev/null ||
	fail "no $target build for $version at $url (releases: https://github.com/$repo/releases)"
curl -fsSL -o "$tmp/$name.tar.gz.sha256" "$url.sha256" 2>/dev/null ||
	fail "could not download $url.sha256"

if command -v sha256sum >/dev/null 2>&1; then
	(cd "$tmp" && sha256sum -c "$name.tar.gz.sha256" >/dev/null)
else
	(cd "$tmp" && shasum -a 256 -c "$name.tar.gz.sha256" >/dev/null)
fi || fail "checksum mismatch for $name.tar.gz"

tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
mkdir -p "$dir" || fail "could not create $dir"
dir=$(cd "$dir" && pwd)
install -m 755 "$tmp/$name/agora" "$dir/agora" || fail "could not write $dir/agora"
say "installed $version to $dir/agora"

case ":$PATH:" in
*":$dir:"*) ;;
*) say "$dir is not on PATH; run it as $dir/agora or add: export PATH=\"$dir:\$PATH\"" ;;
esac
echo "$dir/agora"
