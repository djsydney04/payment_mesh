#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cargo build --release -p mesh-ffi --target aarch64-apple-ios
cargo build --release -p mesh-ffi --target aarch64-apple-ios-sim
mkdir -p ios/Frameworks
mkdir -p target/mesh-headers
cp ios/MeshPay/MeshCore.h target/mesh-headers/MeshCore.h
rm -rf ios/Frameworks/MeshCore.xcframework
xcodebuild -create-xcframework \
  -library target/aarch64-apple-ios/release/libmesh_ffi.a -headers target/mesh-headers \
  -library target/aarch64-apple-ios-sim/release/libmesh_ffi.a -headers target/mesh-headers \
  -output ios/Frameworks/MeshCore.xcframework
