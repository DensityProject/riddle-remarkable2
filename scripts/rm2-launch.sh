#!/bin/sh
# AppLoad entry point for the windowed (qtfb) flavour on reMarkable 1/2.
# AppLoad sets QTFB_KEY; we load oracle.env and hand over to riddle, which
# draws inside an xochitl window. Close the diary from AppLoad.
HERE=$(cd "$(dirname "$0")" && pwd)
if [ -f "$HERE/oracle.env" ]; then
    set -a; . "$HERE/oracle.env"; set +a
fi

# Fresh session on every open (RIDDLE_FRESH_SESSION=0 to keep memories):
# file the previous session's pages away instead of deleting them, so Tom
# starts blank but nothing is lost.
if [ "${RIDDLE_FRESH_SESSION:-1}" != "0" ]; then
    MEM=${RIDDLE_MEMORY_DIR:-/home/root/riddle-data/memories}
    if [ -d "$MEM" ] && [ -n "$(ls -A "$MEM" 2>/dev/null)" ]; then
        ARCHIVE=$(dirname "$MEM")/past-sessions
        mkdir -p "$ARCHIVE" && mv "$MEM" "$ARCHIVE/$(date +%Y%m%d-%H%M%S)"
    fi
fi

cd "$HERE"
HOME=/home/root exec "$HERE/riddle"
