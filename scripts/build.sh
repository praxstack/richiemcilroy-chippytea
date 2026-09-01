#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

# Development retains the configured stable identity (or explicit ad-hoc
# fallback). Distribution must opt into timestamped Developer ID signing.
signing_config=".swiftpm/chippytea/signing-identity"
if [[ -n "${CHIPPYTEA_SIGNING_IDENTITY:-}" ]]; then
    signing_identity="$CHIPPYTEA_SIGNING_IDENTITY"
elif [[ -e "$signing_config" || -L "$signing_config" ]]; then
    if [[ ! -f "$signing_config" || ! -r "$signing_config" ]]; then
        printf 'Cannot read local signing configuration: %s\n' "$signing_config" >&2
        exit 1
    fi
    signing_identity="$(< "$signing_config")"
    if [[ -z "$signing_identity" || "$signing_identity" == *$'\n'* ]]; then
        printf 'Local signing configuration must contain one nonempty identity.\n' >&2
        exit 1
    fi
else
    signing_identity="-"
fi
if [[ "${CHIPPYTEA_RELEASE:-0}" == "1" && "$signing_identity" == "-" ]]; then
    printf 'A release cannot use ad-hoc signing. Set CHIPPYTEA_SIGNING_IDENTITY.\n' >&2
    exit 1
fi

export MACOSX_DEPLOYMENT_TARGET=14.0
export CFLAGS="${CFLAGS:-} -mmacosx-version-min=14.0"
app_output="${CHIPPYTEA_APP_OUTPUT:-build/chippytea.app}"
swift_build_path="${CHIPPYTEA_SWIFT_BUILD_PATH:-.build}"
cargo_target_dir="${CARGO_TARGET_DIR:-target}"
host_arch="$(uname -m)"
read -r -a architectures <<< "${CHIPPYTEA_ARCHS:-$host_arch}"
if [[ "${#architectures[@]}" -eq 0 ]]; then
    printf 'At least one build architecture is required.\n' >&2
    exit 1
fi
for arch in "${architectures[@]}"; do
    case "$arch" in arm64|x86_64) ;; *) printf 'Unsupported architecture: %s\n' "$arch" >&2; exit 1 ;; esac
done

mkdir -p "$(dirname "$app_output")"
staging="$(mktemp -d "$(dirname "$app_output")/.chippytea-build.XXXXXX")"
trap 'rm -rf "$staging"' EXIT
staged_app="$staging/chippytea.app"
mkdir -p "$staged_app/Contents/MacOS" "$staged_app/Contents/Resources" "$staged_app/Contents/Frameworks"
executables=()
framework_source=""
for arch in "${architectures[@]}"; do
    cargo_args=(build --release --locked)
    # SwiftPM's build database is shared inside a scratch path. Switching its
    # architecture between incremental runs can reuse the wrong command graph.
    swift_arch_build_path="$swift_build_path/$arch"
    swift_args=(--scratch-path "$swift_arch_build_path" -c release --disable-sandbox --arch "$arch")
    rust_dir="$cargo_target_dir/release"
    # Preserve the existing host CLI location for local benchmarks. Explicit
    # architecture builds use Rust's per-target output, including universal CI.
    if [[ -n "${CHIPPYTEA_ARCHS:-}" || "$arch" != "$host_arch" ]]; then
        case "$arch" in arm64) triple="aarch64-apple-darwin" ;; x86_64) triple="x86_64-apple-darwin" ;; esac
        cargo_args+=(--target "$triple")
        rust_dir="$cargo_target_dir/$triple/release"
    fi
    cargo "${cargo_args[@]}"
    CHIPPYTEA_RUST_LIB_DIR="$(cd "$rust_dir" && pwd)"
    export CHIPPYTEA_RUST_LIB_DIR
    swift_bin_dir="$(swift build "${swift_args[@]}" --show-bin-path)"
    rust_digest="$(shasum -a 256 "$rust_dir/libchippytea_core.a" | cut -d ' ' -f 1)"
    rust_stamp="$swift_bin_dir/.chippytea-rust.sha256"
    # SwiftPM cannot observe changes to an external static archive. Invalidate
    # just the executable when its Rust input changes; keep cached Swift objects.
    if [[ ! -f "$rust_stamp" ]] || [[ "$(cat "$rust_stamp")" != "$rust_digest" ]]; then
        rm -f "$swift_bin_dir/chippytea"
    fi
    swift build "${swift_args[@]}" --force-resolved-versions
    printf '%s\n' "$rust_digest" > "$rust_stamp"
    printf '%s %s\n' "$arch" "$rust_digest" >> "$staged_app/Contents/Resources/engine-build.sha256"
    cp "$swift_bin_dir/chippytea" "$staging/chippytea-$arch"
    # SwiftPM adds a build-machine rpath for its binary dependency. Only bundle
    # or system-relative rpaths may survive into the distributable executable.
    python3 - "$staging/chippytea-$arch" <<'PY'
import re, subprocess, sys
binary = sys.argv[1]
load_commands = subprocess.check_output(['otool', '-l', binary], text=True)
for path in re.findall(r'cmd LC_RPATH\s+cmdsize \d+\s+path (.+?) \(offset', load_commands):
    if not path.startswith('@') and not path.startswith('/usr/lib/'):
        subprocess.run(['install_name_tool', '-delete_rpath', path, binary], check=True)
PY
    executables+=("$staging/chippytea-$arch")
    if [[ -z "$framework_source" ]]; then
        framework_source="$swift_bin_dir/Sparkle.framework"
        if [[ ! -d "$framework_source" ]]; then
            framework_source="$swift_arch_build_path/artifacts/sparkle/Sparkle/Sparkle.xcframework/macos-arm64_x86_64/Sparkle.framework"
        fi
        if [[ ! -d "$framework_source" ]]; then
            printf 'SwiftPM did not provide the pinned Sparkle framework.\n' >&2
            exit 1
        fi
    fi
done
if [[ "${#executables[@]}" -eq 1 ]]; then
    cp "${executables[0]}" "$staged_app/Contents/MacOS/chippytea"
else
    lipo -create "${executables[@]}" -output "$staged_app/Contents/MacOS/chippytea"
fi
chmod +x "$staged_app/Contents/MacOS/chippytea"
ditto "$framework_source" "$staged_app/Contents/Frameworks/Sparkle.framework"
cp native/Info.plist "$staged_app/Contents/Info.plist"
cp native/Assets/AppIcon.icns "$staged_app/Contents/Resources/AppIcon.icns"
python3 - "$staged_app/Contents/Info.plist" <<'PY'
import os, plistlib, re, sys
path = sys.argv[1]
with open(path, 'rb') as source:
    info = plistlib.load(source)
for env, key in [('CHIPPYTEA_BUILD_VERSION', 'CFBundleShortVersionString'), ('CHIPPYTEA_BUILD_NUMBER', 'CFBundleVersion')]:
    if env in os.environ:
        value = os.environ[env]
        if not re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', value):
            raise SystemExit(f'{env} must be a numeric major.minor.patch version.')
        info[key] = value
with open(path, 'wb') as output:
    plistlib.dump(info, output, sort_keys=False)
PY
bash scripts/release/sign-app.sh "$staged_app" "$signing_identity"

# Replace only the requested build output after all linking and signing pass.
# The temporary directory belongs solely to this invocation.
if [[ -e "$app_output" || -L "$app_output" ]]; then
    mv "$app_output" "$staging/previous.app"
fi
mv "$staged_app" "$app_output"
printf 'Built %s (%s; verified signature)\n' "$app_output" "${architectures[*]}"
