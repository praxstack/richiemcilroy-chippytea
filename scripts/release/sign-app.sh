#!/bin/bash
set -euo pipefail

app="${1:?Usage: sign-app.sh App.app identity}"
identity="${2:?A signing identity is required}"
framework="$app/Contents/Frameworks/Sparkle.framework"
sign=(--force --sign "$identity")
if [[ -n "${CHIPPYTEA_SIGNING_KEYCHAIN:-}" ]]; then
    sign+=(--keychain "$CHIPPYTEA_SIGNING_KEYCHAIN")
fi
if [[ "${CHIPPYTEA_RELEASE:-0}" == "1" ]]; then
    if [[ "$identity" == "-" ]]; then
        printf 'Release builds require a Developer ID Application identity.\n' >&2
        exit 1
    fi
    sign+=(--options runtime --timestamp)
else
    sign+=(--timestamp=none)
fi

# Sign from the inside out. Keep Sparkle's downloader sandbox entitlements,
# but not the upstream team's designated requirements. Never sign with --deep.
for component in \
    "Versions/B/XPCServices/Downloader.xpc" \
    "Versions/B/XPCServices/Installer.xpc" \
    "Versions/B/Autoupdate" \
    "Versions/B/Updater.app"; do
    if [[ ! -e "$framework/$component" ]]; then
        printf 'The pinned Sparkle framework is missing %s\n' "$component" >&2
        exit 1
    fi
    codesign "${sign[@]}" --preserve-metadata=entitlements "$framework/$component"
done
codesign "${sign[@]}" "$framework"
codesign "${sign[@]}" "$app"
codesign --verify --deep --strict --verbose=2 "$app"

if [[ "${CHIPPYTEA_RELEASE:-0}" == "1" ]]; then
    codesign --display --verbose=4 "$app" 2>&1 | python3 -c '
import sys
details = sys.stdin.read()
if "Authority=Developer ID Application:" not in details or "runtime" not in details or "Timestamp=" not in details:
    raise SystemExit("Release signature must be timestamped Developer ID with hardened runtime.")
print("Verified timestamped Developer ID signature and hardened runtime.")
'
fi
