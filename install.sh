#!/bin/sh
# Build and install for the current user (default) or under another prefix:
#   ./install.sh            -> ~/.local
#   sudo ./install.sh /usr  -> system wide
#   ./install.sh /app       -> in a Flatpak build
set -e
prefix="${1:-$HOME/.local}"
id=io.github.DrakRosmann.CosmicExtAppletQuickSettings
bin=cosmic-ext-applet-quick-settings

cargo build --release
install -Dm0755 "target/release/$bin" "$prefix/bin/$bin"
mkdir -p "$prefix/share/applications"
sed "s|@bindir@|$prefix/bin|" "data/$id.desktop.in" > "$prefix/share/applications/$id.desktop"
install -Dm0644 "data/$id.metainfo.xml" "$prefix/share/metainfo/$id.metainfo.xml"
install -Dm0644 "data/icons/scalable/apps/$id.svg" "$prefix/share/icons/hicolor/scalable/apps/$id.svg"
echo "Installed. Add \"Quick Settings\" in Settings > Desktop > Panel > Applets."
