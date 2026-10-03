#!/bin/sh
# Load the udev rule for /dev/uinput (Wayland paste key). No udev, for
# example in a container, is not a failure.
if command -v udevadm >/dev/null 2>&1; then
  udevadm control --reload >/dev/null 2>&1 || true
  udevadm trigger --subsystem-match=misc --sysname-match=uinput >/dev/null 2>&1 || true
fi
exit 0
