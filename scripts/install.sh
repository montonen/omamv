#!/usr/bin/env bash
# Build omamv and install it for the current user: the binary, a link to it
# that launchers can see, the desktop entry and the icon. Safe to run again,
# which is also how to update.
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
app_id=se.montonen.Omamv

if [[ $(id -u) -eq 0 ]]; then
  echo "install.sh: run it as yourself, not as root; it installs into your home" >&2
  exit 1
fi
if ! command -v cargo >/dev/null; then
  echo "install.sh: cargo not found; on Arch: sudo pacman -S --needed rust" >&2
  exit 1
fi

cargo install --path "$repo"

# cargo puts the binary in ~/.cargo/bin, which a Hyprland/uwsm session's PATH
# does not include: without this link the desktop entry and any keybinding
# cannot find it, though a shell can.
cargo_bin="${CARGO_INSTALL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}/bin/omamv"
mkdir -p "$HOME/.local/bin"
ln -sfn "$cargo_bin" "$HOME/.local/bin/omamv"

data="${XDG_DATA_HOME:-$HOME/.local/share}"
install -Dm644 "$repo/data/$app_id.desktop" "$data/applications/$app_id.desktop"
install -Dm644 "$repo/data/$app_id.svg" "$data/icons/hicolor/scalable/apps/$app_id.svg"

# A convenience; launchers find the entry without it. No icon cache: a user
# icon folder normally has none, and GTK reads the folder itself. Building
# one there failed validation and left a temp file behind.
if command -v update-desktop-database >/dev/null; then
  update-desktop-database -q "$data/applications" || true
fi

echo "Installed:"
echo "  $cargo_bin"
echo "  $HOME/.local/bin/omamv -> $cargo_bin"
echo "  $data/applications/$app_id.desktop"
echo "  $data/icons/hicolor/scalable/apps/$app_id.svg"
