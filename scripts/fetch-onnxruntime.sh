#!/usr/bin/env bash
# Download the pinned official ONNX Runtime for a Rust target and place its
# shared library, under the normalised name dreamcore looks for, in <dest-dir>.
#
#   scripts/fetch-onnxruntime.sh <rust-target> <dest-dir>
#
# Used by the release workflows (next to the built binary), the Docker image,
# and by developers: with <dest-dir> = target/onnxruntime a plain cargo build
# finds the library (see crates/dream-domain-devops/src/onnxruntime.rs).
#
# A target with no pinned build (x86_64-apple-darwin) is not an error: nothing
# is written and the exit status is 0, because local embedding is simply
# unavailable there.
#
# Windows archives are .zip; use scripts/fetch-onnxruntime.ps1 for those.
set -euo pipefail

target="${1:?usage: fetch-onnxruntime.sh <rust-target> <dest-dir>}"
dest="${2:?usage: fetch-onnxruntime.sh <rust-target> <dest-dir>}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
pin="$here/onnxruntime-pin.txt"

base_url="$(sed -n 's/^BASE_URL=//p' "$pin")"
line="$(grep -E "^${target} " "$pin" || true)"
if [[ -z "$line" ]]; then
  echo "fetch-onnxruntime: no ONNX Runtime build is pinned for ${target}; skipping (local embedding unavailable there)"
  exit 0
fi
read -r _ archive sha256 <<<"$line"

case "$archive" in
  *.tgz) ;;
  *) echo "fetch-onnxruntime: ${archive} is not a .tgz; use fetch-onnxruntime.ps1" >&2; exit 2 ;;
esac

case "$target" in
  *-apple-darwin) wanted="libonnxruntime.dylib" ;;
  *) wanted="libonnxruntime.so" ;;
esac

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "fetch-onnxruntime: downloading ${archive}"
curl -fsSL --retry 5 --retry-delay 3 -o "$work/$archive" "${base_url}/${archive}"
actual="$(sha256_of "$work/$archive")"
if [[ "$actual" != "$sha256" ]]; then
  echo "fetch-onnxruntime: sha256 mismatch for ${archive}: expected ${sha256}, got ${actual}" >&2
  exit 1
fi

tar -xzf "$work/$archive" -C "$work"
# The package ships libonnxruntime.so -> .so.1 -> .so.<version> symlinks; copy
# the resolved file so the result is one self-contained, plainly named library.
src="$(find "$work" -path '*/lib/*' -name "$wanted" | head -1)"
if [[ -z "$src" ]]; then
  echo "fetch-onnxruntime: ${wanted} not found inside ${archive}" >&2
  exit 1
fi
mkdir -p "$dest"
cp -L "$src" "$dest/$wanted"
echo "fetch-onnxruntime: wrote $dest/$wanted ($(wc -c <"$dest/$wanted") bytes)"
