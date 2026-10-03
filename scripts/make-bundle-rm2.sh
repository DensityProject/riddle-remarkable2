#!/usr/bin/env bash
# Stage the reMarkable 1/2 windowed AppLoad bundle into dist/riddle-rm2/.
# Prereq: scripts/build-rm2.sh has produced the ARMv7 binary.
set -euo pipefail
cd "$(dirname "$0")/.."

BIN=target/armv7-unknown-linux-gnueabihf/release/riddle
[ -f "$BIN" ] || { echo "build first: scripts/build-rm2.sh" >&2; exit 1; }

rm -rf dist/riddle-rm2
mkdir -p dist/riddle-rm2
install -m 755 "$BIN" dist/riddle-rm2/riddle
install -m 755 scripts/rm2-launch.sh dist/riddle-rm2/
install -m 644 rm2/external.manifest.json icon.png oracle.env.example dist/riddle-rm2/

echo "staged: $(du -sh dist/riddle-rm2 | cut -f1) in dist/riddle-rm2/"
echo "install: scp -O -r dist/riddle-rm2 root@10.11.99.1:/home/root/xovi/exthome/appload/riddle"
