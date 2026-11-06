#!/bin/sh
# Start the JACK bridge (serves the page too) and open it. Extra args go to pluto-viz,
# e.g. ./run.sh --connect system:capture_3 --connect system:capture_4
cd "$(dirname "$0")"
BIN=bridge/target/release/pluto-viz
[ -x "$BIN" ] || cargo build --release --manifest-path bridge/Cargo.toml || exit 1
"$BIN" "$@" &
trap 'kill $!' EXIT INT TERM
sleep 0.5
xdg-open "http://127.0.0.1:8765/" >/dev/null 2>&1
wait
