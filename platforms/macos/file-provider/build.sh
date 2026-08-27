#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
project_dir="$root/platforms/macos/file-provider"
derived_data="$root/target/macos-file-provider-derived"
staging="$root/target/macos-file-provider"
version="$(node -p "require('$root/package.json').version")"

command -v xcodegen >/dev/null || {
  echo "xcodegen is required to build the macOS File Provider extension" >&2
  exit 1
}

xcodegen generate --spec "$project_dir/project.yml" --project "$project_dir"
rm -rf "$derived_data" "$staging"
mkdir -p "$staging"

xcodebuild build \
  -project "$project_dir/BifrostFileProvider.xcodeproj" \
  -scheme BifrostFileProvider \
  -configuration Release \
  -derivedDataPath "$derived_data" \
  CODE_SIGNING_ALLOWED=YES \
  CODE_SIGN_IDENTITY="${APPLE_SIGNING_IDENTITY:--}" \
  MARKETING_VERSION="$version" \
  CURRENT_PROJECT_VERSION="$version" \
  ONLY_ACTIVE_ARCH=YES

products="$derived_data/Build/Products/Release"
cp -R "$products/BifrostFileProvider.appex" "$staging/BifrostFileProvider.appex"
cp "$products/libBifrostFileProviderHostBridge.dylib" "$staging/libBifrostFileProviderHostBridge.dylib"

test -x "$staging/BifrostFileProvider.appex/Contents/MacOS/BifrostFileProvider"
test -f "$staging/libBifrostFileProviderHostBridge.dylib"
codesign --verify --strict --verbose=2 "$staging/BifrostFileProvider.appex"
