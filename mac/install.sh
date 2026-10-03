#!/bin/sh
# Install (or update) the Mac side: the Claude bridge and the tablet
# auto-link watcher, both as launchd agents. Safe to re-run.
#
#   mac/install.sh            # installs into ~/diary-bridge, port 8788
#   BRIDGE_PORT=8787 mac/install.sh
set -eu
SRC=$(cd "$(dirname "$0")" && pwd)
DEST=${BRIDGE_DIR:-$HOME/diary-bridge}
PORT=${BRIDGE_PORT:-8788}
PY=${PYTHON:-$(command -v python3)}
ME=$(id -un)
AGENTS=$HOME/Library/LaunchAgents

mkdir -p "$DEST/empty" "$AGENTS"
for f in bridge.py tablet_link.py; do
    # Keep the previous copy rather than overwrite it blindly.
    [ -f "$DEST/$f" ] && ! cmp -s "$SRC/$f" "$DEST/$f" && cp "$DEST/$f" "$DEST/$f.prev"
    cp "$SRC/$f" "$DEST/$f"
done
if [ ! -s "$DEST/token" ]; then
    (umask 077; openssl rand -hex 32 > "$DEST/token")
    echo "new bridge token in $DEST/token (plug the tablet in to send it over)"
fi
chmod 600 "$DEST/token"

plist() { # label script log extra-env-xml
    [ -f "$AGENTS/$1.plist" ] && cp "$AGENTS/$1.plist" "$DEST/$1.plist.prev"
    cat > "$AGENTS/$1.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key><string>$1</string>
	<key>ProgramArguments</key>
	<array><string>$PY</string><string>$DEST/$2</string></array>
	<key>EnvironmentVariables</key>
	<dict>
		<key>BRIDGE_PORT</key><string>$PORT</string>
		<key>PATH</key><string>$HOME/.local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>$4
	</dict>
	<key>WorkingDirectory</key><string>$DEST</string>
	<key>StandardOutPath</key><string>$DEST/$3</string>
	<key>StandardErrorPath</key><string>$DEST/$3</string>
	<key>RunAtLoad</key><true/>
	<key>KeepAlive</key><true/>
	<key>ThrottleInterval</key><integer>15</integer>
</dict>
</plist>
EOF
    launchctl bootout "gui/$(id -u)/$1" 2>/dev/null || true
    launchctl bootstrap "gui/$(id -u)" "$AGENTS/$1.plist"
}

# The tablet's Wi-Fi address is added to $DEST/allow by the watcher.
plist "com.$ME.diary-bridge" bridge.py bridge.log '
		<key>BRIDGE_ALLOW</key><string>10.11.99.1</string>'
plist "com.$ME.tablet-link" tablet_link.py link.log ''
echo "installed: bridge on :$PORT and tablet-link watcher ($DEST)"
