#!/bin/sh
# AppLoad entry point for the windowed (qtfb) flavour on reMarkable 1/2.
# AppLoad sets QTFB_KEY; we load oracle.env and hand over to riddle, which
# draws inside an xochitl window. Close the diary from AppLoad.
HERE=$(cd "$(dirname "$0")" && pwd)
if [ -f "$HERE/oracle.env" ]; then
    set -a; . "$HERE/oracle.env"; set +a
fi
cd "$HERE"
HOME=/home/root exec "$HERE/riddle"
