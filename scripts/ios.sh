#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
action="${1:-build}"
destination="${MESH_IOS_DESTINATION:-generic/platform=iOS Simulator}"
if [[ "$action" != build && "$action" != test ]]; then
  echo 'Usage: scripts/ios.sh [build|test]' >&2
  exit 1
fi
signing=(CODE_SIGNING_ALLOWED=YES CODE_SIGN_IDENTITY=- CODE_SIGNING_REQUIRED=NO)
if [[ "$destination" != *'platform=iOS Simulator'* ]]; then
  config="${MESH_IOS_CONFIG:-$PWD/.local/banking/ios.xcconfig}"
  if [[ ! -f "$config" ]]; then
    echo 'Run scripts/configure-banking.py first, or set MESH_IOS_CONFIG to your local signing configuration.' >&2
    exit 1
  fi
  signing=(-xcconfig "$config" CODE_SIGNING_ALLOWED=YES CODE_SIGNING_REQUIRED=YES CODE_SIGN_STYLE=Automatic)
fi
if [[ "$action" = test && "$destination" = 'generic/platform=iOS Simulator' ]]; then
  echo 'Set MESH_IOS_DESTINATION="platform=iOS Simulator,id=YOUR-SIMULATOR-UUID" to run UI tests.' >&2
  exit 1
fi
./scripts/fetch-plaid.sh
./scripts/build-rust-ios.sh
python3 scripts/generate-xcode.py
xcodebuild -project ios/MeshPay.xcodeproj -scheme MeshPay -destination "$destination" \
  -derivedDataPath ios/build "${signing[@]}" "$action"
