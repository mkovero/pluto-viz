#!/bin/sh
# Kiosk start: the JACK bridge fed from desktop audio (PipeWire's JACK ports; change the
# --connect ports to your source), shown in a Brave app window with its own profile and
# process, so a GPU reset in the everyday browser (or in this one) does not leave WebGL
# blocked. Copy next to a built pluto-viz binary. F = fullscreen.
cd "$(dirname "$0")"
pkill -x pluto-viz
pkill -f -- "--user-data-dir=$PWD/brave-profile"
setsid nohup ./pluto-viz --connect PipeWire:playback_FL --connect PipeWire:playback_FR \
  >pluto-viz.log 2>&1 </dev/null &
sleep 1
DISPLAY=${DISPLAY:-:0} setsid nohup brave --user-data-dir="$PWD/brave-profile" \
  --ignore-gpu-blocklist --disable-gpu-process-crash-limit --no-first-run \
  --app=http://127.0.0.1:8765/ >brave.log 2>&1 </dev/null &
