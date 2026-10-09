#!/bin/sh
# Install what building the applet needs, and the programs it runs, with the
# system's package manager (apt, dnf or pacman), if anything is missing; and
# check that Rust is as new as libcosmic asks.
set -e

# libcosmic's rust-version.
rust=1.93

missing=
for tool in git cc pkg-config cargo wpctl pw-dump pactl curl; do
    command -v "$tool" >/dev/null 2>&1 || missing="$missing $tool"
done
if command -v pkg-config >/dev/null 2>&1; then
    for lib in xkbcommon wayland-client; do
        pkg-config --exists "$lib" || missing="$missing $lib"
    done
fi

if [ -n "$missing" ]; then
    echo "Missing:$missing"
    if command -v apt-get >/dev/null 2>&1; then
        # Debian, Ubuntu, Pop!_OS, Mint...
        sudo apt-get install -y git build-essential pkgconf cargo rustc \
            libxkbcommon-dev libwayland-dev wireplumber pipewire-bin pulseaudio-utils curl
    elif command -v dnf >/dev/null 2>&1; then
        # Fedora
        sudo dnf install -y git gcc pkgconf-pkg-config cargo rust \
            libxkbcommon-devel wayland-devel wireplumber pipewire-utils pulseaudio-utils curl
    elif command -v pacman >/dev/null 2>&1; then
        # Arch, Manjaro, EndeavourOS...
        rustpkg=rust
        # rustup provides cargo too; don't swap one for the other.
        command -v rustup >/dev/null 2>&1 && rustpkg=
        sudo pacman -S --needed --noconfirm git base-devel pkgconf $rustpkg \
            libxkbcommon wayland wireplumber pipewire libpulse curl
    else
        echo "Unknown package manager: install these yourself." >&2
        exit 1
    fi
fi

# Distributions often ship an older Rust than libcosmic asks for.
have=$(rustc --version | cut -d' ' -f2)
if [ "$(printf '%s\n%s\n' "$rust" "$have" | sort -V | head -n1)" != "$rust" ]; then
    if command -v rustup >/dev/null 2>&1; then
        rustup update stable
    else
        echo "Rust $have is too old: $rust or newer is needed. Install it with rustup" >&2
        echo "(https://rustup.rs), then run this again." >&2
        exit 1
    fi
fi
