#!/usr/bin/env bash
# The basics, the way a person starts omamv: the debug binary on its own (no
# self-check holding it up), offscreen on Broadway, with OMAMV_DEBUG's trace
# read back. Each case is a fresh process.
#
#     scripts/smoke.sh       exit 0 when every case passes
set -uo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
work="$repo/target/smoke"
cd "$repo"
cargo build --quiet || exit 1
bin="$repo/target/debug/omamv"

mkdir -p "$work"
run_id=$(date +%s%N)
case_dir="$work/$run_id"
mkdir -p "$case_dir/state" "$case_dir/empty/src"
cp -r tests/fixture "$case_dir/fx"
printf 'fn main() {}\n' > "$case_dir/empty/src/main.rs"

display=""
for n in $(seq 61 80); do
  [ -e "${XDG_RUNTIME_DIR:-/tmp}/broadway$((n + 1)).socket" ] && continue
  display=$n; break
done
[ -n "$display" ] || { echo "smoke.sh: no free Broadway display" >&2; exit 1; }
gtk4-broadwayd ":$display" >/dev/null 2>&1 &
broadway=$!
trap 'kill $broadway 2>/dev/null' EXIT
sleep 0.5

fails=0
pass() { echo "ok   $1"; }
fail() { echo "FAIL $1"; fails=$((fails + 1)); }

# omamv ARGS... for a few seconds; the trace in $out.
launch() {
  out="$case_dir/trace-$1.txt"; shift
  env -i HOME="$HOME" PATH="$PATH" XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp}" \
    XDG_STATE_HOME="$case_dir/state" GDK_BACKEND=broadway BROADWAY_DISPLAY=":$display" \
    DBUS_SESSION_BUS_ADDRESS=disabled: OMAMV_ALONE=1 OMAMV_DEBUG=1 \
    timeout "${WAIT:-6}" "$bin" "$@" > "$out" 2>&1
  status=$?
}
has() { grep -qF -- "$1" "$out"; }

launch folder "$case_dir/fx"
has 'load omamv://w1/README.md' && has '"ready":true' && pass "a folder opens its README, drawn" || fail "a folder opens its README, drawn"
[ "$status" -eq 124 ] && pass "  and stays open" || fail "  and stays open (exit $status)"

launch file "$case_dir/fx/docs/architecture.md"
has 'load omamv://w1/architecture.md' && has '"ready":true' && pass "a file opens in its folder" || fail "a file opens in its folder"

launch empty "$case_dir/empty"
has 'scan done: 0 files' && has 'nothing to show' && pass "a folder with no Markdown says so" || fail "a folder with no Markdown says so"

launch repo "$repo"
has 'load omamv://w1/README.md' && has '"ready":true' && pass "this repository opens (target/ and .git skipped)" || fail "this repository opens"

launch start
[ "$status" -eq 124 ] && pass "no folder: the start screen stays open" || fail "no folder: start screen (exit $status)"

"$bin" --help > /dev/null 2>&1 && pass "--help" || fail "--help"
"$bin" --version | grep -q '^omamv ' && pass "--version" || fail "--version"
"$bin" /no/such/place > /dev/null 2>&1; [ $? -eq 1 ] && pass "a missing path is refused" || fail "a missing path is refused"
"$bin" --bogus > /dev/null 2>&1; [ $? -eq 1 ] && pass "an unknown option is refused" || fail "an unknown option is refused"

echo "smoke: $fails failed (traces in $case_dir)"
[ "$fails" -eq 0 ]
