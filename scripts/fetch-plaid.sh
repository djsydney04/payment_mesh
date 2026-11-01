#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
version=7.2.2
checksum=d02ece9d8a621f3ed34f02e19ec1ebfc982071b299f748d94d5719b88029ab8b
archive=".local/LinkKit-$version.zip"
mkdir -p .local ios/Frameworks
if [ ! -f "$archive" ]; then
  curl -fL --retry 2 --max-time 120 \
    "https://github.com/plaid/plaid-link-ios/releases/download/$version/LinkKit.xcframework.zip" \
    -o "$archive.tmp"
  mv "$archive.tmp" "$archive"
fi
actual=$(shasum -a 256 "$archive" | cut -d ' ' -f 1)
if [ "$actual" != "$checksum" ]; then
  echo "Plaid SDK checksum mismatch; refusing to use this archive." >&2
  exit 1
fi
if [ ! -d ios/Frameworks/LinkKit.xcframework ]; then
  unzip -q "$archive" -d ios/Frameworks
fi
