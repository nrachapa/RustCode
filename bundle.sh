#!/bin/sh
# Assemble an unsigned (ad-hoc signed) RustCode.app from the release binary.
set -eu

cd "$(dirname "$0")"
BIN=target/release/rustcode
APP=RustCode.app

[ -x "$BIN" ] || { echo "Build first: cargo build --release"; exit 1; }

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp "$BIN" "$APP/Contents/MacOS/rustcode"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>RustCode</string>
    <key>CFBundleDisplayName</key>
    <string>RustCode</string>
    <key>CFBundleExecutable</key>
    <string>rustcode</string>
    <key>CFBundleIdentifier</key>
    <string>dev.rustcode.editor</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

# Ad-hoc signature so macOS treats it as a signed (local) app.
codesign --force --sign - "$APP"
echo "Built $APP (unsigned: right-click → Open on first run)."
