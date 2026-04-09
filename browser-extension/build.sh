#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

rm -rf dist
mkdir -p dist/chrome dist/firefox

# Chrome
cp manifest.json content.js content.css dist/chrome/
cp -r icons dist/chrome/
(cd dist/chrome && zip -r ../relevant-reviews-chrome.zip .)

# Firefox
cp manifest-firefox.json dist/firefox/manifest.json
cp content.js content.css dist/firefox/
cp -r icons dist/firefox/
(cd dist/firefox && zip -r ../relevant-reviews-firefox.xpi .)

echo "Built:"
echo "  dist/relevant-reviews-chrome.zip"
echo "  dist/relevant-reviews-firefox.xpi"
