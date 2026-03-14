#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 3 ]]; then
  echo "usage: $0 <package-version> <binary-path> <output-dir>" >&2
  exit 1
fi

PACKAGE_VERSION="$1"
BINARY_PATH="$2"
OUTPUT_DIR="$3"
PACKAGE_NAME="rolodex"
ARCH="amd64"
STAGE_DIR="$(mktemp -d)"
PKG_ROOT="$STAGE_DIR/${PACKAGE_NAME}_${PACKAGE_VERSION}_${ARCH}"
trap 'rm -rf "$STAGE_DIR"' EXIT

mkdir -p "$PKG_ROOT/DEBIAN" "$PKG_ROOT/usr/lib/rolodex" "$PKG_ROOT/usr/bin" "$OUTPUT_DIR"
install -m 0755 "$BINARY_PATH" "$PKG_ROOT/usr/lib/rolodex/riff-codex"
if command -v strip >/dev/null 2>&1; then
  strip --strip-unneeded "$PKG_ROOT/usr/lib/rolodex/riff-codex" || true
fi
cat > "$PKG_ROOT/usr/bin/rolodex" <<'WRAP'
#!/usr/bin/env bash
set -euo pipefail
exec /usr/lib/rolodex/riff-codex "$@"
WRAP
chmod 0755 "$PKG_ROOT/usr/bin/rolodex"
cat > "$PKG_ROOT/DEBIAN/control" <<CONTROL
Package: ${PACKAGE_NAME}
Version: ${PACKAGE_VERSION}
Section: utils
Priority: optional
Architecture: ${ARCH}
Maintainer: Riff Labs <hello@riff.cc>
Description: Rolodex (based on Codex)
 Rolodex is the Riff Labs Codex-derived agent interface and terminal environment.
CONTROL

dpkg-deb --root-owner-group --build "$PKG_ROOT" "$OUTPUT_DIR/${PACKAGE_NAME}_${PACKAGE_VERSION}_${ARCH}.deb"
