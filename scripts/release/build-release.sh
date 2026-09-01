#!/bin/bash
# Build and validate public artifacts. Publication is a separate final step.
set -euo pipefail
set +x
umask 077
cd "$(dirname "$0")/../.."

if [[ "$#" -ne 2 ]]; then
    printf 'Usage: %s VERSION EMPTY_OUTPUT_DIRECTORY\n' "$0" >&2
    exit 2
fi
version="$1"
output="$2"
: "${CHIPPYTEA_RELEASE_PLAN:?Provide the validated release plan path}"

python3 -B - "$version" "$output" "$CHIPPYTEA_RELEASE_PLAN" <<'PY'
import json, pathlib, sys
sys.path.insert(0, "scripts/release")
import release
plan = release.read_plan(pathlib.Path(sys.argv[3]))
release.require(plan["version"] == sys.argv[1], "Version differs from the release plan.")
output = pathlib.Path(sys.argv[2])
release.require(output.is_absolute() and not output.is_symlink(),
                "Release output must be an absolute, non-symlink directory.")
output.mkdir(parents=True, exist_ok=True)
release.require(not any(output.iterdir()), "Release output directory must be empty.")
PY

work="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/chippytea-release.XXXXXX")"
keychain="$work/signing.keychain-db"
mounted=""
cleanup() {
    status="$?"
    trap - EXIT
    if [[ -n "$mounted" ]]; then
        hdiutil detach "$mounted" -force >/dev/null 2>&1 || true
    fi
    security delete-keychain "$keychain" >/dev/null 2>&1 || true
    rm -rf -- "$work"
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

log_dir="${CHIPPYTEA_RELEASE_LOG_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/chippytea-release-logs}"
mkdir -p "$log_dir"
certificate="$work/certificate.p12"
ed_key="$work/sparkle-private-key"
asc_key="$work/AuthKey.p8"

# Keep secret values out of arguments and logs while decoding them. Only the
# native keychain/notary import tools receive passwords as required by their CLI.
python3 -B - "$certificate" "$ed_key" "$asc_key" <<'PY'
import base64, os, pathlib, re, sys
required = ("APPLE_CERTIFICATE_P12_BASE64", "APPLE_CERTIFICATE_PASSWORD",
            "APPLE_SIGNING_IDENTITY", "APPLE_TEAM_ID", "SPARKLE_PRIVATE_KEY")
for name in required:
    if not os.environ.get(name):
        sys.exit(f"Missing release secret/configuration: {name}")
if not re.fullmatch(r"[A-Z0-9]{10}", os.environ["APPLE_TEAM_ID"]):
    sys.exit("APPLE_TEAM_ID must be a 10-character Apple team ID.")
if not os.environ["APPLE_SIGNING_IDENTITY"].startswith("Developer ID Application:"):
    sys.exit("APPLE_SIGNING_IDENTITY must be a Developer ID Application identity.")
try:
    certificate = base64.b64decode(os.environ["APPLE_CERTIFICATE_P12_BASE64"], validate=True)
    seed_text = os.environ["SPARKLE_PRIVATE_KEY"].strip()
    seed = base64.b64decode(seed_text, validate=True)
except ValueError:
    sys.exit("Certificate or Sparkle secret is not valid base64.")
if not certificate or len(seed) != 32:
    sys.exit("Certificate must be nonempty and Sparkle key must be a 32-byte private seed.")
pathlib.Path(sys.argv[1]).write_bytes(certificate)
pathlib.Path(sys.argv[2]).write_text(seed_text + "\n")
api_names = ("ASC_KEY_ID", "ASC_ISSUER_ID", "ASC_PRIVATE_KEY")
if any(os.environ.get(name) for name in api_names):
    if not all(os.environ.get(name) for name in api_names):
        sys.exit("Provide all three App Store Connect notarization secrets, or none.")
    pathlib.Path(sys.argv[3]).write_text(os.environ["ASC_PRIVATE_KEY"])
elif not os.environ.get("APPLE_ID") or not os.environ.get("APPLE_APP_SPECIFIC_PASSWORD"):
    sys.exit("Provide Apple ID notarization credentials or the complete ASC key set.")
PY

keychain_password="$(openssl rand -hex 32)"
security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$certificate" -k "$keychain" -P "$APPLE_CERTIFICATE_PASSWORD" \
    -T /usr/bin/codesign -T /usr/bin/security >/dev/null
security set-key-partition-list -S apple-tool:,apple:,codesign: \
    -s -k "$keychain_password" "$keychain" >/dev/null

notary_profile="chippytea-release"
if [[ -f "$asc_key" ]]; then
    xcrun notarytool store-credentials "$notary_profile" --keychain "$keychain" \
        --key "$asc_key" --key-id "$ASC_KEY_ID" --issuer "$ASC_ISSUER_ID" \
        --validate >"$work/notary-credentials.log" 2>&1 || {
        printf 'App Store Connect notarization authentication failed.\n' >&2
        exit 1
    }
else
    xcrun notarytool store-credentials "$notary_profile" --keychain "$keychain" \
        --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
        --password "$APPLE_APP_SPECIFIC_PASSWORD" \
        --validate >"$work/notary-credentials.log" 2>&1 || {
        printf 'Apple ID notarization authentication failed.\n' >&2
        exit 1
    }
fi
unset APPLE_CERTIFICATE_P12_BASE64 APPLE_CERTIFICATE_PASSWORD SPARKLE_PRIVATE_KEY
unset APPLE_APP_SPECIFIC_PASSWORD ASC_PRIVATE_KEY
rm -f -- "$certificate" "$asc_key"

sparkle_bin="$(python3 -B scripts/release/release.py tools --destination "$work/tools")"
archives="$work/appcast"
python3 -B scripts/release/release.py prepare --plan "$CHIPPYTEA_RELEASE_PLAN" --archives "$archives"
previous_args=()
if [[ -f "$archives/appcast.xml" ]]; then
    "$sparkle_bin/sign_update" --verify --ed-key-file "$ed_key" "$archives/appcast.xml"
    cp "$archives/appcast.xml" "$work/previous-appcast.xml"
    previous_args=(--previous "$work/previous-appcast.xml")
fi

app="$work/app/chippytea.app"
CHIPPYTEA_ARCHS="arm64 x86_64" \
CHIPPYTEA_BUILD_VERSION="$version" \
CHIPPYTEA_BUILD_NUMBER="$version" \
CHIPPYTEA_APP_OUTPUT="$app" \
CHIPPYTEA_SIGNING_IDENTITY="$APPLE_SIGNING_IDENTITY" \
CHIPPYTEA_SIGNING_KEYCHAIN="$keychain" \
CHIPPYTEA_RELEASE=1 \
CHIPPYTEA_SWIFT_BUILD_PATH="$work/swift" \
CARGO_TARGET_DIR="$work/cargo" \
    env -u GH_TOKEN -u GITHUB_TOKEN bash scripts/build.sh

python3 -B scripts/release/release.py validate-plist \
    --plist "$app/Contents/Info.plist" --version "$version"
lipo -verify_arch arm64 x86_64 "$app/Contents/MacOS/chippytea"
# Build-directory overrides also change the scanner's ownership rules. Keep
# them, along with publishing tokens, out of disposable native test processes.
env -u GH_TOKEN -u GITHUB_TOKEN -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET_DIR \
    "$app/Contents/MacOS/chippytea" --self-test
env -u GH_TOKEN -u GITHUB_TOKEN -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET_DIR \
    "$app/Contents/MacOS/chippytea" --update-self-test
codesign --display --verbose=4 "$app" 2>"$work/codesign.txt"
python3 -B - "$work/codesign.txt" "$APPLE_TEAM_ID" <<'PY'
import pathlib, sys
details = pathlib.Path(sys.argv[1]).read_text().splitlines()
if f"TeamIdentifier={sys.argv[2]}" not in details:
    sys.exit("Signed app belongs to the wrong Apple team.")
if not any(line.startswith("Authority=Developer ID Application:") for line in details):
    sys.exit("Signed app is not Developer ID signed.")
if not any(line.startswith("Timestamp=") for line in details):
    sys.exit("Signed app has no secure timestamp.")
if not any(line.startswith("CodeDirectory ") and "runtime" in line for line in details):
    sys.exit("Signed app is missing Hardened Runtime.")
PY

# The private key is never imported into the login keychain. CryptoKit derives
# its public key and compares it with the exact app that is being shipped.
xcrun swift - "$app/Contents/Info.plist" "$ed_key" <<'SWIFT'
import CryptoKit
import Foundation
func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(1)
}
do {
    let infoData = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))
    guard let info = try PropertyListSerialization.propertyList(
        from: infoData, options: [], format: nil) as? [String: Any],
        let expected = info["SUPublicEDKey"] as? String else {
        fail("The app has no Sparkle public key.")
    }
    let encoded = try String(contentsOfFile: CommandLine.arguments[2], encoding: .utf8)
        .trimmingCharacters(in: .whitespacesAndNewlines)
    guard let seed = Data(base64Encoded: encoded), seed.count == 32 else {
        fail("Invalid Sparkle signing key format.")
    }
    let key = try Curve25519.Signing.PrivateKey(rawRepresentation: seed)
    guard key.publicKey.rawRepresentation.base64EncodedString() == expected else {
        fail("Sparkle private key does not match the app's public key.")
    }
    print("Sparkle signing key matches the app.")
} catch {
    fail("Sparkle key verification failed.")
}
SWIFT

notarize() {
    local archive="$1" result="$2"
    printf 'Submitting %s for Apple notarization…\n' "$(basename "$archive")"
    xcrun notarytool submit "$archive" --keychain-profile "$notary_profile" \
        --keychain "$keychain" --wait --timeout 30m --output-format json >"$result"
    python3 -B - "$result" <<'PY'
import json, pathlib, sys
result = json.loads(pathlib.Path(sys.argv[1]).read_text())
if result.get("status") != "Accepted":
    sys.exit("Apple did not accept the notarization. Inspect the notarization JSON artifact.")
print("Apple notarization accepted.")
PY
}

verify_app() {
    codesign --verify --deep --strict --verbose=2 "$1"
    xcrun stapler validate "$1"
    spctl --assess --type execute --verbose=2 "$1"
    lipo -verify_arch arm64 x86_64 "$1/Contents/MacOS/chippytea"
    python3 -B scripts/release/release.py validate-plist \
        --plist "$1/Contents/Info.plist" --version "$version"
}

ditto -c -k --sequesterRsrc --keepParent "$app" "$work/notarization.zip"
notarize "$work/notarization.zip" "$log_dir/notary-app.json"
xcrun stapler staple "$app"
verify_app "$app"

zip="$output/chippytea-$version-universal.zip"
ditto -c -k --sequesterRsrc --keepParent "$app" "$zip"
python3 -B scripts/release/release.py validate-zip --archive "$zip" --version "$version"
mkdir "$work/recovered"
ditto -x -k "$zip" "$work/recovered"
verify_app "$work/recovered/chippytea.app"

mkdir "$work/dmg-root"
ditto "$app" "$work/dmg-root/chippytea.app"
ln -s /Applications "$work/dmg-root/Applications"
dmg="$output/chippytea-$version-universal.dmg"
hdiutil create -volname chippytea -srcfolder "$work/dmg-root" -fs APFS -format ULFO "$dmg"
codesign --force --sign "$APPLE_SIGNING_IDENTITY" --keychain "$keychain" --timestamp "$dmg"
codesign --verify --strict "$dmg"
notarize "$dmg" "$log_dir/notary-dmg.json"
xcrun stapler staple "$dmg"
xcrun stapler validate "$dmg"
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
mkdir "$work/mounted"
hdiutil attach "$dmg" -readonly -nobrowse -mountpoint "$work/mounted" >"$work/mount.log"
mounted="$work/mounted"
[[ -L "$mounted/Applications" && "$(readlink "$mounted/Applications")" == "/Applications" ]]
verify_app "$mounted/chippytea.app"
hdiutil detach "$mounted"
mounted=""

cp "$zip" "$archives/"
cp "$archives/chippytea-$version-universal.md" "$output/release-notes.md"
"$sparkle_bin/generate_appcast" --ed-key-file "$ed_key" \
    --versions "$version" --maximum-versions 0 --maximum-deltas 0 \
    --embed-release-notes \
    --download-url-prefix "https://github.com/richiemcilroy/chippytea/releases/download/v$version/" \
    --full-release-notes-url "https://github.com/richiemcilroy/chippytea/releases/tag/v$version" \
    --link "https://github.com/richiemcilroy/chippytea" "$archives"
signature="$(python3 -B scripts/release/release.py validate-appcast \
    --feed "$archives/appcast.xml" --archive "$zip" --version "$version" \
    "${previous_args[@]}")"
"$sparkle_bin/sign_update" --verify --ed-key-file "$ed_key" "$zip" "$signature"
"$sparkle_bin/sign_update" --verify --ed-key-file "$ed_key" "$archives/appcast.xml"
cp "$archives/appcast.xml" "$output/appcast.xml"
python3 -B scripts/release/release.py finish --plan "$CHIPPYTEA_RELEASE_PLAN" --artifacts "$output"
printf 'All release artifacts verified. Nothing has been published yet.\n'
