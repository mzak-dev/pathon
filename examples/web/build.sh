#!/bin/sh
# Builds the web example into examples/web/pkg (index.html loads it from there).
set -e
cd "$(dirname "$0")"
wasm-pack build --release --target web
