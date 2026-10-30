#!/usr/bin/env bash
set -euo pipefail

# Ubuntu 26.04 provides libdisplay-info 0.3, required by the pinned Smithay.
if (( EUID != 0 )); then
    exec sudo bash "$0" "$@"
fi

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install --no-install-recommends -y \
    build-essential \
    clang \
    dbus \
    libclang-dev \
    libdisplay-info-dev \
    libegl1-mesa-dev \
    libgbm-dev \
    libinput-dev \
    libpipewire-0.3-dev \
    libseat-dev \
    libsystemd-dev \
    libudev-dev \
    libwayland-dev \
    libx11-dev \
    libxi-dev \
    libxkbcommon-dev \
    libxkbcommon-x11-dev \
    pkg-config
