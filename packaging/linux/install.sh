#!/bin/sh
# Install Sayso for the current user (default) or for everyone (--system).
#
#   ./install.sh            ~/.local/bin, ~/.local/share
#   sudo ./install.sh --system   /usr/local/bin, /usr/local/share, and the udev rule
#   ./install.sh --uninstall     remove the user install
set -eu
here=$(cd "$(dirname "$0")" && pwd)
mode=user
case "${1:-}" in
  --system) mode=system ;;
  --uninstall) mode=uninstall ;;
  "") ;;
  *) echo "usage: $0 [--system | --uninstall]" >&2; exit 2 ;;
esac

if [ "$mode" = system ]; then
  bin=/usr/local/bin; lib=/usr/local/lib/sayso; share=/usr/local/share
else
  bin="$HOME/.local/bin"; lib="$HOME/.local/lib/sayso"; share="${XDG_DATA_HOME:-$HOME/.local/share}"
fi

if [ "$mode" = uninstall ]; then
  rm -rf "$lib" "$bin/sayso" "$share/applications/dev.sayso.Sayso.desktop" "$share/metainfo/dev.sayso.Sayso.metainfo.xml"
  find "$share/icons/hicolor" -name dev.sayso.Sayso.png -delete 2>/dev/null || true
  echo "Sayso is removed. Your settings and history stay in ~/.config/sayso and ~/.local/share/sayso."
  exit 0
fi

mkdir -p "$bin" "$lib" "$share/applications" "$share/metainfo"
# The app finds the engine next to itself, so both live in one folder.
install -m 755 "$here/bin/sayso" "$lib/sayso"
install -m 755 "$here/bin/sayso-engine" "$lib/sayso-engine"
ln -sf "$lib/sayso" "$bin/sayso"
sed "s|^Exec=sayso|Exec=$lib/sayso|" "$here/share/applications/dev.sayso.Sayso.desktop" > "$share/applications/dev.sayso.Sayso.desktop"
cp "$here/share/metainfo/dev.sayso.Sayso.metainfo.xml" "$share/metainfo/"
for dir in "$here"/share/icons/hicolor/*/apps; do
  size=$(basename "$(dirname "$dir")")
  mkdir -p "$share/icons/hicolor/$size/apps"
  cp "$dir/dev.sayso.Sayso.png" "$share/icons/hicolor/$size/apps/"
done
command -v update-desktop-database >/dev/null && update-desktop-database "$share/applications" >/dev/null 2>&1 || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$share/icons/hicolor" >/dev/null 2>&1 || true

if [ "$mode" = system ]; then
  install -m 644 "$here/lib/udev/rules.d/70-sayso-uinput.rules" /etc/udev/rules.d/
  udevadm control --reload >/dev/null 2>&1 && udevadm trigger >/dev/null 2>&1 || true
fi

echo "Sayso is installed. Start it from your app menu, or run: sayso"
case ":$PATH:" in *":$bin:"*) ;; *) echo "Note: $bin is not on your PATH." ;; esac
