#!/usr/bin/env bash
#
# download-stats.sh — show download numbers for Marrow across all channels.
#
#   crates.io        : marrow-cli / marrow-core (public API, no auth)
#   GitHub releases  : per-asset download counts (covers direct downloads AND
#                      Homebrew installs, which fetch the release tarballs)
#
# Homebrew note: a custom tap (besendorfer/tap) has NO install analytics of its
# own — only homebrew-core formulae appear on formulae.brew.sh/analytics. So the
# GitHub *-apple-darwin.tar.gz count below is the best proxy for `brew install`s.
#
# Requires: gh (authenticated), curl, python3.
# Usage:    ./scripts/download-stats.sh

set -euo pipefail

REPO="Besendorfer/marrow"
UA="marrow-download-stats (teancum@besendorfer.net)"

bold() { printf '\033[1m%s\033[0m\n' "$1"; }

bold "== crates.io =="
for crate in marrow-cli marrow-core; do
  curl -fsSL -A "$UA" "https://crates.io/api/v1/crates/${crate}" \
    | python3 -c '
import sys, json
d = json.load(sys.stdin)
c = d["crate"]
print("  {:<13} total {:>6}   (90d {})".format(c["name"], c["downloads"], c.get("recent_downloads") or 0))
for v in d["versions"]:
    print("      {:<16} {:>6}".format(v["num"], v["downloads"]))
' || echo "  ${crate}: not published / fetch failed"
done

echo
bold "== GitHub release assets (direct downloads + Homebrew) =="
# Sum per asset across every release; tag the Apple-Silicon CLI tarball as the
# Homebrew/Mac proxy. Skip checksum/formula bookkeeping files.
gh api "repos/${REPO}/releases" --paginate \
  --jq '.[] | .tag_name as $t | .assets[] | "\(.download_count)\t\($t)\t\(.name)"' \
  | python3 -c '
import sys
rows = [l.rstrip("\n").split("\t") for l in sys.stdin if l.strip()]
total = 0
for count, tag, name in sorted(rows, key=lambda r: r[1], reverse=True):
    if name in ("SHA256SUMS", "marrow.rb", "latest.json"):
        continue
    n = int(count)
    total += n
    tag_proxy = " <- Mac CLI (brew + direct)" if name.endswith("aarch64-apple-darwin.tar.gz") else ""
    print(f"  {n:>4}  {tag:<20} {name}{tag_proxy}")
print(f"  ----  {total} total asset downloads (excl. checksums/formula/updater)")
'
