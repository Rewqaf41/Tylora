#!/bin/sh
# Checks a built Tylora.app and Tylora.dmg: layout, versions, architecture
# and code signatures. Pass --release to also require a Developer ID
# signature, a notarization ticket and Gatekeeper acceptance (release gate).
set -eu

root=$(cd "$(dirname "$0")/../../.." && pwd)
bundle="$root/target/release/bundle"
app="$bundle/macos/Tylora.app"
release=false
[ "${1:-}" = "--release" ] && release=true
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "ok:   $*"; }

[ -d "$app" ] || fail "missing $app"
plist="$app/Contents/Info.plist"
id=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$plist")
name=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleName' "$plist")
version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$plist")
[ "$id" = "com.tylora.settings" ] || fail "bundle identifier is $id"
[ "$name" = "Tylora" ] || fail "bundle name is $name"
ok "Info.plist: $name $version ($id)"

expected=$(node -p "require('$root/apps/gui/package.json').version")
[ "$version" = "$expected" ] || fail "bundle version $version, package.json $expected"
daemon="$app/Contents/MacOS/tylora"
[ -x "$daemon" ] || fail "bundled daemon missing at Contents/MacOS/tylora"
daemon_version=$("$daemon" --version | awk '{print $2}')
[ "$daemon_version" = "$version" ] || fail "daemon version $daemon_version, app $version"
ok "daemon Contents/MacOS/tylora $daemon_version"
[ -f "$app/Contents/Resources/icon.icns" ] || fail "icon.icns missing"
ok "icon.icns present"

for binary in "$app/Contents/MacOS/"*; do
	ok "$(basename "$binary"): $(lipo -archs "$binary")"
done

codesign --verify --deep --strict --verbose=2 "$app" || fail "codesign verification"
codesign -d --verbose=2 "$daemon" 2>&1 | grep -q 'flags=.*runtime' || fail "daemon lacks hardened runtime"
codesign -d --verbose=2 "$app" 2>&1 | grep -q 'flags=.*runtime' || fail "app lacks hardened runtime"
ok "signatures valid, hardened runtime on app and daemon"

dmg=$(ls "$bundle"/dmg/Tylora_*.dmg 2>/dev/null | head -n 1 || true)
if [ -n "$dmg" ]; then
	hdiutil verify -quiet "$dmg" || fail "dmg checksum"
	ok "dmg $(basename "$dmg") $(du -h "$dmg" | cut -f1)"
else
	echo "note: no dmg found"
fi

if $release; then
	authority=$(codesign -d --verbose=2 "$app" 2>&1 | sed -n 's/^Authority=//p' | head -n 1)
	case "$authority" in "Developer ID Application:"*) ok "signed by $authority" ;; *) fail "not Developer ID signed (${authority:-ad-hoc})" ;; esac
	xcrun stapler validate "$app" || fail "app has no stapled notarization ticket"
	spctl --assess --type execute --verbose "$app" || fail "Gatekeeper rejects the app"
	[ -n "$dmg" ] || fail "release needs a dmg"
	xcrun stapler validate "$dmg" || fail "dmg has no stapled notarization ticket"
	spctl --assess --type open --context context:primary-signature --verbose "$dmg" || fail "Gatekeeper rejects the dmg"
	ok "release gate passed"
fi
