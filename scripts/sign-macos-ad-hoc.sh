#!/bin/bash

set -euo pipefail

app_path="${1:?Usage: sign-macos-ad-hoc.sh /path/to/GpuTerm.app}"

if [[ ! -d "$app_path" || "$app_path" != *.app ]]; then
  echo "Expected a macOS .app bundle: $app_path" >&2
  exit 1
fi

sign_macho() {
  local item="$1"
  if file -b "$item" | grep -q "Mach-O"; then
    codesign --force --sign - --timestamp=none "$item"
  fi
}

# Sign every executable/library first. This covers the main binary and any
# nested helpers or dylibs that a future dependency may add to the bundle.
while IFS= read -r -d '' item; do
  sign_macho "$item"
done < <(find "$app_path/Contents" -type f -print0)

# Sign nested code containers from the deepest path outward before applying
# the final recursive signature to the top-level application bundle.
while IFS= read -r -d '' item; do
  codesign --force --sign - --timestamp=none "$item"
done < <(
  find "$app_path/Contents" -depth -type d \
    \( -name '*.framework' -o -name '*.xpc' -o -name '*.appex' \
       -o -name '*.plugin' -o -name '*.bundle' -o -name '*.app' \) \
    -print0
)

codesign --force --deep --sign - --timestamp=none "$app_path"
codesign --verify --deep --strict --verbose=4 "$app_path"
codesign --display --verbose=4 "$app_path"
