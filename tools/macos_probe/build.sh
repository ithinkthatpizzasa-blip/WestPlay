#!/bin/sh
# Builds WestPlay's macOS probe plugin (probe.cpp) for the game's architecture (x86_64), ad-hoc signed.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
sdk="$here/../scs_sdk_1_15/include"
out="${1:-$here/westplay_probe.dylib}"
clang++ -std=c++17 -arch x86_64 -dynamiclib -O2 -fvisibility=hidden -Wno-deprecated-declarations \
    -I "$sdk" -framework OpenGL -o "$out" "$here/probe.cpp"
codesign --force -s - "$out"
echo "$out"
