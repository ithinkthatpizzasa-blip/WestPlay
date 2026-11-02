#!/bin/sh
# Builds WestPlay's plugin for the Mac game (x86_64: the game runs under Rosetta, docs/MACOS_PORT.md), release,
# ad-hoc signed (setup tells the installed plugin by its bytes, so it's signed here, not on the way in), as
# target/release/westplay.dylib: where the companion takes it from while developing (app/src/setup.rs).
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
cargo build -p westplay-plugin --release --target x86_64-apple-darwin
mkdir -p target/release
cp target/x86_64-apple-darwin/release/libwestplay.dylib target/release/westplay.dylib
install_name_tool -id @rpath/westplay.dylib target/release/westplay.dylib
codesign --force -s - target/release/westplay.dylib
echo "$root/target/release/westplay.dylib"
