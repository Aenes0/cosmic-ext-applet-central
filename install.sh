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
for entry in "$id" "$id.ShowDesktop" "$id.Start"; do
    sed "s|@bindir@|$prefix/bin|" "data/$entry.desktop.in" > "$prefix/share/applications/$entry.desktop"
done
install -Dm0644 "data/$id.metainfo.xml" "$prefix/share/metainfo/$id.metainfo.xml"
install -Dm0644 "data/icons/scalable/apps/$id.svg" "$prefix/share/icons/hicolor/scalable/apps/$id.svg"
echo "Installed. Add \"Central\" in Settings > Desktop > Panel > Applets."
