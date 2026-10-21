#!/bin/sh
# Puts the Start menu in place of COSMIC's launcher and app library: the
# Super key (and Super+A, Super+/) opens it, and its button takes the place
# of the launcher's and the library's in the panel.
#   ./start-menu.sh          -> use the Start menu
#   ./start-menu.sh --undo   -> back to COSMIC's own
set -e
bin="${BIN:-$HOME/.local/bin/cosmic-ext-applet-central}"
cfg="${XDG_CONFIG_HOME:-$HOME/.config}/cosmic"
actions="$cfg/com.system76.CosmicSettings.Shortcuts/v1/system_actions"
center="$cfg/com.system76.CosmicPanel.Panel/v1/plugins_center"
start=io.github.Aenes0.CosmicCentral.Start

if [ "$1" = "--undo" ]; then
    rm -f "$actions"
    [ -f "$actions.bak" ] && mv "$actions.bak" "$actions"
    if [ -f "$center" ]; then
        sed -i "s|\"$start\",|\"com.system76.CosmicPanelAppButton\",\n    \"com.system76.CosmicPanelLauncherButton\",|" "$center"
    fi
    pkill -f "cosmic-ext-applet-central --start$" || true
    echo "COSMIC's launcher and app library are back."
    exit 0
fi

# The compositor reads these and replaces COSMIC's own, key by key.
mkdir -p "$(dirname "$actions")"
[ -f "$actions" ] && [ ! -f "$actions.bak" ] && cp "$actions" "$actions.bak"
cat > "$actions" <<EOF
{
    Launcher: "$bin --start",
    AppLibrary: "$bin --start",
}
EOF

# One button for both in the panel, where the launcher's was.
if [ -f "$center" ] && ! grep -q "\"$start\"" "$center"; then
    sed -i -e "/\"com.system76.CosmicPanelAppButton\",/d" \
        -e "s|\"com.system76.CosmicPanelLauncherButton\",|\"$start\",|" "$center"
    grep -q "\"$start\"" "$center" || sed -i "s|^Some(\[|Some([\n    \"$start\",|" "$center"
fi
echo "Start menu set: press Super."
