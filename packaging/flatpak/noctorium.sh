#!/bin/sh
# Starts Noctorium inside its sandbox. The finish-args already point NOCTORIUM_MPV_PATH at the mpv built
# into this Flatpak -- the host's is out of reach in here -- and this says it again for anybody who runs
# the launcher with a cleared environment.
export NOCTORIUM_MPV_PATH="${NOCTORIUM_MPV_PATH:-/app/bin/mpv}"
exec /app/noctorium/bin/Noctorium "$@"
