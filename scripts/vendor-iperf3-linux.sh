#!/usr/bin/env sh
set -eu

VERSION="${1:-3.21}"
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PROJECT_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
ARCH=$(uname -m)
case "$ARCH" in
  x86_64|aarch64) ;;
  *) echo "Unsupported Linux architecture: $ARCH" >&2; exit 1 ;;
esac

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT INT TERM
curl -fL "https://github.com/esnet/iperf/archive/refs/tags/${VERSION}.tar.gz" -o "$WORK/iperf.tar.gz"
tar -xzf "$WORK/iperf.tar.gz" -C "$WORK"
SOURCE="$WORK/iperf-${VERSION}"
cd "$SOURCE"
./configure --enable-static-bin --without-sctp
make -j"$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 2)"

DESTINATION="$PROJECT_ROOT/src-tauri/resources/iperf3/linux-${ARCH}"
mkdir -p "$DESTINATION"
cp "$SOURCE/src/iperf3" "$DESTINATION/iperf3"
cp "$SOURCE/LICENSE" "$DESTINATION/LICENSE-IPERF3.txt"
chmod 755 "$DESTINATION/iperf3"
"$DESTINATION/iperf3" --version
echo "Bundled iperf3 $VERSION at $DESTINATION"
