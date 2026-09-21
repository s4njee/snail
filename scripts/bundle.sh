#!/usr/bin/env bash
#
# Build Snail in release and bundle it into `dist/Snail.app` (plan.md E18.1).
#
# macOS only: the `.app`, the `.icns` and the ad-hoc signature are macOS concepts. The app's fonts
# and icons are embedded in the binary with `include_bytes!`, so the bundle carries only the
# executable, the Info.plist and the icon — no loose assets to keep in sync.
#
# Ad-hoc signing is enough for personal use (no notarization). If that ever stops being true, the
# change is: a Developer ID certificate, `codesign --options runtime` with entitlements, then
# `xcrun notarytool submit` and `xcrun stapler staple`.
#
# The bundle is also what makes new-mail notifications work (plan.md E16.8): macOS delivers them
# only to an app with a bundle id, so under `cargo run` they are dropped. The Dock badge works
# either way. Launch with `--test-notification` to check delivery (the first one asks permission).
#
# Usage: scripts/bundle.sh [--install]
#   --install   also quit any running Snail, replace /Applications/Snail.app with this build, and
#               re-register it with Launch Services so the Dock and Notification Center use it.
set -euo pipefail

INSTALL=0
for arg in "$@"; do
    case "$arg" in
        --install) INSTALL=1 ;;
        -h | --help)
            sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "unknown option: $arg (try --help)" >&2
            exit 1
            ;;
    esac
done

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="Snail"
BUNDLE_ID="dev.snail.app"
EXECUTABLE="snail"
ICON_SRC="$ROOT/appicon.jpg"
DIST="$ROOT/dist"
APP="$DIST/$APP_NAME.app"

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "bundle.sh builds a macOS .app (this is $(uname -s))." >&2
    echo "On Linux/Windows use: cargo build --release, then run target/release/snail." >&2
    exit 1
fi

for tool in cargo sips iconutil codesign; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "required tool not found: $tool" >&2
        exit 1
    }
done

[[ -f "$ICON_SRC" ]] || {
    echo "dock icon not found: $ICON_SRC" >&2
    exit 1
}

# The workspace version (`[workspace.package] version = "…"`), so the bundle and the crate agree.
VERSION="$(awk -F'"' '/^\[workspace\.package\]/{found=1} found && /^version/{print $2; exit}' "$ROOT/Cargo.toml")"
VERSION="${VERSION:-0.0.0}"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

echo "==> building $EXECUTABLE $VERSION (release)"
cargo build --release --locked --bin "$EXECUTABLE" --manifest-path "$ROOT/Cargo.toml"

BIN="$TARGET_DIR/release/$EXECUTABLE"
[[ -x "$BIN" ]] || {
    echo "build did not produce $BIN" >&2
    exit 1
}

echo "==> assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/$EXECUTABLE"

# A JPEG cannot be a bundle icon, so render the sizes iconutil expects and pack them to .icns.
ICONSET_ROOT="$(mktemp -d)"
trap 'rm -rf "$ICONSET_ROOT"' EXIT
ICONSET="$ICONSET_ROOT/AppIcon.iconset"
mkdir -p "$ICONSET"
render() { sips -s format png -z "$2" "$2" "$ICON_SRC" --out "$ICONSET/$1" >/dev/null; }
render icon_16x16.png 16
render icon_16x16@2x.png 32
render icon_32x32.png 32
render icon_32x32@2x.png 64
render icon_128x128.png 128
render icon_128x128@2x.png 256
render icon_256x256.png 256
render icon_256x256@2x.png 512
render icon_512x512.png 512
render icon_512x512@2x.png 1024
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleDisplayName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleExecutable</key>
    <string>${EXECUTABLE}</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>${APP_NAME}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.productivity</string>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSHumanReadableCopyright</key>
    <string>Snail</string>
</dict>
</plist>
PLIST

printf 'APPL????' > "$APP/Contents/PkgInfo"

echo "==> ad-hoc signing"
codesign --force --sign - "$APP"
codesign --verify --verbose=2 "$APP"

echo
echo "bundled: $APP"

if [[ "$INSTALL" == 1 ]]; then
    INSTALLED="/Applications/$APP_NAME.app"
    if pgrep -f "$INSTALLED/Contents/MacOS/$EXECUTABLE" >/dev/null 2>&1; then
        echo "==> quitting the running $APP_NAME"
        osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
        for _ in $(seq 1 50); do
            pgrep -f "$INSTALLED/Contents/MacOS/$EXECUTABLE" >/dev/null 2>&1 || break
            sleep 0.1
        done
        if pgrep -f "$INSTALLED/Contents/MacOS/$EXECUTABLE" >/dev/null 2>&1; then
            echo "$APP_NAME did not quit; quit it and run again" >&2
            exit 1
        fi
    fi
    echo "==> installing to $INSTALLED"
    rm -rf "$INSTALLED"
    ditto "$APP" "$INSTALLED"
    # So the Dock, Spotlight and Notification Center pick up this build rather than a cached one.
    LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
    [[ -x "$LSREGISTER" ]] && "$LSREGISTER" -f "$INSTALLED"
    echo "installed: $INSTALLED"
    echo "run it with: open \"$INSTALLED\""
else
    echo "run it with: open \"$APP\"  (or re-run with --install to replace /Applications/$APP_NAME.app)"
fi
