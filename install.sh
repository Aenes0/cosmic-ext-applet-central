#!/bin/sh
# Build and install for the current user (default) or under another prefix:
#   ./install.sh            -> ~/.local
#   sudo ./install.sh /usr  -> system wide
#   ./install.sh /app       -> in a Flatpak build
set -e
prefix="${1:-$HOME/.local}"
id=io.github.Aenes0.CosmicCentral
bin=cosmic-ext-applet-central

# In a Flatpak build the manifest brings what's needed.
[ -f /.flatpak-info ] || "$(dirname "$0")/deps.sh"
cargo build --release
install -Dm0755 "target/release/$bin" "$prefix/bin/$bin"
mkdir -p "$prefix/share/applications"
# The tray keeps the id it had as its own program, and with it its place on the panel.
for entry in "$id" "$id.ShowDesktop" "$id.Monitor" io.github.Aenes0.CosmicTray; do
    sed "s|@bindir@|$prefix/bin|" "data/$entry.desktop.in" > "$prefix/share/applications/$entry.desktop"
done
install -Dm0644 "data/$id.metainfo.xml" "$prefix/share/metainfo/$id.metainfo.xml"
install -Dm0644 "data/icons/scalable/apps/$id.svg" "$prefix/share/icons/hicolor/scalable/apps/$id.svg"
# The clipboard service is a user unit, also activated over D-Bus; both files point at the same binary.
mkdir -p "$prefix/share/systemd/user" "$prefix/share/dbus-1/services"
sed "s|@bindir@|$prefix/bin|" "data/central-clipboard.service.in" > "$prefix/share/systemd/user/central-clipboard.service"
sed "s|@bindir@|$prefix/bin|" "data/io.github.Aenes0.CosmicCentral.Clipboard.service.in" > "$prefix/share/dbus-1/services/io.github.Aenes0.CosmicCentral.Clipboard.service"
# Only a plain ~/.local install enables the unit here; Flatpak builds and system prefixes leave that to the packager.
if [ "$prefix" = "$HOME/.local" ] && [ ! -f /.flatpak-info ]; then
    systemctl --user daemon-reload && systemctl --user enable central-clipboard.service && systemctl --user restart central-clipboard.service || true
fi
echo "Installed. Add \"Central\" in Settings > Desktop > Panel > Applets."
