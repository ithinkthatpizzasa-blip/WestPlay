#!/bin/sh
# Builds WestPlay for the Mac (`npm run build:mac`): the plugin (tools/build_plugin_macos.sh), then the companion as
# one universal WestPlay.app (Apple silicon and Intel) with the plugin in its Resources, its .dmg, and the updater's
# WestPlay.app.tar.gz. Ad-hoc signed, for this Mac; with APPLE_SIGNING_IDENTITY (and Apple's account for
# notarization) signed for everyone: docs/MACOS_PORT.md, "Shipping on the Mac". Extra arguments go to `tauri build`.
#
# Then checks the plugin went in byte for byte: setup tells the installed plugin by its bytes, so a plugin signed
# again on the way into the bundle would be "another version" to it.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
sh tools/build_plugin_macos.sh
# The updater's archive is signed with WestPlay's private key, the release's to give (TAURI_SIGNING_PRIVATE_KEY):
# without it Tauri makes everything, then fails on the signature. A build without the key leaves the archive out.
if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
    echo "No TAURI_SIGNING_PRIVATE_KEY: building without the updater's archive (a release has the key)." >&2
    set -- --config '{"bundle":{"createUpdaterArtifacts":false}}' "$@"
fi
npx --no tauri build --config app/tauri.release.macos.conf.json --target universal-apple-darwin "$@"
bundle=target/universal-apple-darwin/release/bundle
app=$bundle/macos/WestPlay.app
if ! cmp -s target/release/westplay.dylib "$app/Contents/Resources/westplay.dylib"; then
    echo "The plugin in $app isn't the one built: setup would take it for another version." >&2
    exit 1
fi
codesign --verify --deep --strict "$app"
ls -d "$app" "$bundle"/dmg/*.dmg
ls "$bundle"/macos/*.tar.gz* 2>/dev/null || true
