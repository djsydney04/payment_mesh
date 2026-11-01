#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
action="${1:-build}"
./scripts/fetch-plaid.sh
./scripts/build-rust-ios.sh
python3 scripts/generate-xcode.py
destination="${MESH_IOS_DESTINATION:-generic/platform=iOS Simulator}"
if [ "$action" = test ]; then
  if [ "$destination" = 'generic/platform=iOS Simulator' ]; then
    echo 'Set MESH_IOS_DESTINATION="platform=iOS Simulator,id=YOUR-SIMULATOR-UUID" to run UI tests.' >&2
    exit 1
  fi
  xcodebuild -project ios/MeshPay.xcodeproj -scheme MeshPay -destination "$destination" \
    -derivedDataPath ios/build CODE_SIGNING_ALLOWED=YES CODE_SIGN_IDENTITY=- CODE_SIGNING_REQUIRED=NO test
else
  xcodebuild -project ios/MeshPay.xcodeproj -scheme MeshPay -destination "$destination" \
    -derivedDataPath ios/build CODE_SIGNING_ALLOWED=YES CODE_SIGN_IDENTITY=- CODE_SIGNING_REQUIRED=NO build
fi
