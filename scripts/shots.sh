#!/usr/bin/env bash
# Pictures of omamv for the README and montonen.se, offscreen: the debug
# binary on GTK's Broadway display at 2×, with scratch config and state and
# an Omarchy theme, running a plan of steps (src/ui/shots.rs says how one
# reads).
#
#     scripts/shots.sh <folder> <plan> <theme colors.toml> <out dir>
#
# Only ever over public folders: the Omarchy repositories kept in
# ~/.cache/omarepos-shots (see the omarepos-recordings skill). HOME is that
# fleet's home, so paths in the pictures read ~/Work/<repo>.
set -euo pipefail

folder=$(realpath "$1") plan=$(realpath "$2") colors=$(realpath "$3") out=$4
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
fleet_home=${OMAMV_SHOTS_HOME:-$HOME/.cache/omarepos-shots/home}
cd "$repo"
cargo build --quiet
mkdir -p "$out"
out=$(realpath "$out")

work="$repo/target/shots"
rm -rf -- "${work:?}"
mkdir -p "$work"/{state,config,data,cache,runtime}
chmod 700 "$work/runtime"
theme="$work/state/omarchy/current"
mkdir -p "$theme/theme"
cp "$colors" "$theme/theme/colors.toml"
basename "$(dirname "$colors")" > "$theme/theme.name"
mkdir -p "$work/config/gtk-4.0"
printf '[Settings]\ngtk-enable-animations=false\n' > "$work/config/gtk-4.0/settings.ini"
printf 'XDG_PICTURES_DIR="%s"\n' "$work/pictures" > "$work/config/user-dirs.dirs"

display=""
for n in $(seq 61 80); do
  sock="${XDG_RUNTIME_DIR:-/tmp}/broadway$((n + 1)).socket"
  if [ -e "$sock" ] && ! pgrep -f "gtk4-broadwayd :$n\$" >/dev/null; then rm -f -- "$sock"; fi
  [ -e "$sock" ] && continue
  display=$n
  break
done
[ -n "$display" ] || { echo "shots.sh: no free Broadway display" >&2; exit 1; }
gtk4-broadwayd ":$display" >/dev/null 2>&1 &
broadway=$!
trap 'kill $broadway 2>/dev/null || true' EXIT
sleep 0.5

env -i HOME="$fleet_home" PATH="$PATH" LANG="${LANG:-C.UTF-8}" \
  XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp}" \
  XDG_STATE_HOME="$work/state" XDG_CONFIG_HOME="$work/config" \
  XDG_DATA_HOME="$work/data" XDG_CACHE_HOME="$work/cache" \
  GDK_BACKEND=broadway BROADWAY_DISPLAY=":$display" GDK_SCALE=2 GDK_DEBUG=no-portals \
  DBUS_SESSION_BUS_ADDRESS=disabled: \
  OMAMV_SHOTS="$plan" OMAMV_SHOTS_OUT="$out" \
  timeout 300 target/debug/omamv "$folder" 2>&1 | grep '^shots:'
