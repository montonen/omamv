#!/usr/bin/env bash
# The offscreen self-check: builds the debug binary, copies the fixture folder,
# and runs omamv on GTK's Broadway display with scratch XDG folders and a
# scratch Omarchy theme. Nothing reaches the desktop: no compositor, no input
# injection, no D-Bus session. The self-check itself is src/ui/debug.rs.
#
#     scripts/check.sh            exit 0 when every check passes
#     KEEP=1 scripts/check.sh     keep target/check (pictures in report.shots/)
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
work="$repo/target/check"
cd "$repo"
cargo build --quiet

rm -rf "$work"
mkdir -p "$work"/{state,config,data,cache,runtime}
chmod 700 "$work/runtime"
cp -r tests/fixture "$work/fx"
# Left out by the fixture's own .gitignore, so made here rather than committed.
mkdir -p "$work/fx/build" && printf '# Ignored\n' > "$work/fx/build/ignored.md"
# A repository of its own, one commit, for the file's git state (ADR-0022).
git -C "$work/fx" init -q -b main
git -C "$work/fx" add -A
git -C "$work/fx" -c user.name=omamv -c user.email=check@omamv.invalid commit -q -m fixture
theme="$work/state/omarchy/current"
mkdir -p "$theme/theme"
cat > "$theme/theme/colors.toml" <<'TOML'
mode = "dark"
accent = "#7aa2f7"
muted = "#414868"
background = "#1a1b26"
lighter_background = "#24283b"
foreground = "#a9b1d6"
dark_foreground = "#565f89"
bright_foreground = "#c0caf5"
red = "#f7768e"
yellow = "#e0af68"
orange = "#eb927b"
green = "#9ece6a"
cyan = "#449dab"
blue = "#7aa2f7"
magenta = "#ad8ee6"
TOML
echo tokyo-night > "$theme/theme.name"

# A free Broadway display, claimed for as long as this script runs.
display=""
for n in $(seq 40 60); do
  sock="${XDG_RUNTIME_DIR:-/tmp}/broadway$((n + 1)).socket"
  # A server that is gone leaves its socket behind: clear it, then use the slot.
  if [ -e "$sock" ] && ! pgrep -f "gtk4-broadwayd :$n\$" >/dev/null; then rm -f -- "$sock"; fi
  [ -e "$sock" ] && continue
  display=$n
  break
done
[ -n "$display" ] || { echo "check.sh: no free Broadway display" >&2; exit 1; }
gtk4-broadwayd ":$display" >/dev/null 2>&1 &
broadway=$!
trap 'kill $broadway 2>/dev/null || true; [ -n "${KEEP:-}" ] || true' EXIT
sleep 0.5

# Pictures the page saves (Ctrl+Shift+S) land here, never in the real ~/Pictures.
printf 'XDG_PICTURES_DIR="%s"\n' "$work/pictures" > "$work/config/user-dirs.dirs"
mkdir -p "$work/config/gtk-4.0"
printf '[Settings]\ngtk-enable-animations=false\n' > "$work/config/gtk-4.0/settings.ini"

set +e
env -i HOME="$HOME" PATH="$PATH" ${WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS:+WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1} LANG="${LANG:-C.UTF-8}" \
  XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp}" \
  XDG_STATE_HOME="$work/state" XDG_CONFIG_HOME="$work/config" \
  XDG_DATA_HOME="$work/data" XDG_CACHE_HOME="$work/cache" \
  GDK_BACKEND=broadway BROADWAY_DISPLAY=":$display" GDK_DEBUG=no-portals \
  DBUS_SESSION_BUS_ADDRESS=disabled: \
  OMAMV_SELFTEST="$work/report.txt" \
  timeout 120 target/debug/omamv "$work/fx" > "$work/omamv.log" 2>&1
status=$?
set -e

if [ ! -f "$work/report.txt" ]; then
  echo "check.sh: no report (omamv exited $status); last lines of the log:" >&2
  tail -20 "$work/omamv.log" >&2
  exit 1
fi
grep -E '^(ok  |FAIL)' "$work/report.txt"
grep -E '^(failed|took ms):' "$work/report.txt"
fails=$(grep -c '^FAIL' "$work/report.txt" || true)
echo "pictures: $work/report.shots/"
# The basics as a person starts omamv, with nothing holding the window up.
echo "--- smoke"
"$repo/scripts/smoke.sh" || fails=$((fails + 1))
[ "$fails" -eq 0 ]
