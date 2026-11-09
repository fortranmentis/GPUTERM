#!/bin/bash
# Retry transient disk-image service errors, never skip integrity checks.
set -euo pipefail
dmg="${1:?Usage: verify-macos-dmg.sh image.dmg mountpoint}"
mount="${2:?Expected a mountpoint}"
test -f "$dmg"
mkdir -p "$mount"

retry_image() {
  local attempt
  for attempt in 1 2 3 4 5; do
    if "$@"; then return 0; fi
    if [ "$attempt" -lt 5 ]; then sleep 3; fi
  done
  return 1
}

retry_image hdiutil verify "$dmg"
retry_image hdiutil attach "$dmg" -readonly -nobrowse -mountpoint "$mount"
trap 'hdiutil detach "$mount" >/dev/null' EXIT
codesign --verify --deep --strict --verbose=4 "$mount/GpuTerm.app"
hdiutil detach "$mount"
trap - EXIT
