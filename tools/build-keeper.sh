#!/usr/bin/env bash
# Linux and Windows (Git Bash): builds the keeper helper (juggl-custode) that the app bundles next to itself,
# and puts it where Tauri looks for sidecars: src-tauri/binaries/juggl-custode-<target triple>[.exe].
# Run it once before `cargo tauri dev` or `cargo tauri build`. macOS does not need it.
set -euo pipefail
cd "$(dirname "$0")/../src-tauri"
triple="$(rustc -vV | sed -n 's/^host: //p')"
ext=""; [[ "$triple" == *windows* ]] && ext=".exe"
out="binaries/juggl-custode-$triple$ext"
# Tauri checks that every sidecar exists before compiling anything, this one included: start from a placeholder
[ -f "$out" ] || { printf '#!/bin/sh\nexit 0\n' > "$out"; chmod +x "$out"; }
cargo build --release --bin juggl-custode
cp "target/release/juggl-custode$ext" "$out"
echo "ok: $out"
