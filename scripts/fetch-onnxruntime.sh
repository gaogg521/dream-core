#!/usr/bin/env bash
# Fetch the pinned official ONNX Runtime for a Rust target and place its shared
# library, under the normalised name dreamcore looks for, in <dest-dir>.
#
#   scripts/fetch-onnxruntime.sh <rust-target> <dest-dir>
#
# onnxruntime-pin.txt lists one or more sources per target, tried in order; each
# is pinned by sha256, so a source that is down, moved or tampered with is
# skipped and the next one is used. A source is either
#   - a raw library file (our own COS mirror of the exact build we ship), or
#   - an official `onnxruntime` wheel from PyPI (a zip; the library is extracted).
# Needs curl, and unzip for wheels.
#
# Used by the release workflows (next to the built binary), the Docker image,
# and by developers: with <dest-dir> = target/onnxruntime a plain cargo build
# finds the library (see crates/dream-domain-devops/src/onnxruntime.rs).
#
# A target with no pinned build (x86_64-apple-darwin) is not an error: nothing
# is written and the exit status is 0, because local embedding is simply
# unavailable there.
#
# The Windows library is handled by scripts/fetch-onnxruntime.ps1.
set -euo pipefail

target="${1:?usage: fetch-onnxruntime.sh <rust-target> <dest-dir>}"
dest="${2:?usage: fetch-onnxruntime.sh <rust-target> <dest-dir>}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
pin="$here/onnxruntime-pin.txt"

case "$target" in
  *-windows-*) echo "fetch-onnxruntime: ${target} is a Windows target; use fetch-onnxruntime.ps1" >&2; exit 2 ;;
  *-apple-darwin) member='onnxruntime/capi/libonnxruntime.*.dylib'; wanted="libonnxruntime.dylib" ;;
  *) member='onnxruntime/capi/libonnxruntime.so.*'; wanted="libonnxruntime.so" ;;
esac

if ! grep -qE "^${target} " "$pin"; then
  echo "fetch-onnxruntime: no ONNX Runtime build is pinned for ${target}; skipping (local embedding unavailable there)"
  exit 0
fi

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# try_source <url> <sha256>: returns 0 and writes $dest/$wanted, or returns 1 after
# saying why (never exits, so the caller can move on to the next source).
try_source() {
  local url="$1" sha256="$2" file="$work/download"
  rm -rf "$work/download" "$work/out"
  echo "fetch-onnxruntime: trying ${url##*/}"
  if ! curl -fsSL --retry 3 --retry-delay 3 --connect-timeout 20 -o "$file" "$url"; then
    echo "fetch-onnxruntime: download failed: ${url}" >&2
    return 1
  fi
  local actual
  actual="$(sha256_of "$file")"
  if [[ "$actual" != "$sha256" ]]; then
    echo "fetch-onnxruntime: sha256 mismatch for ${url##*/}: expected ${sha256}, got ${actual}" >&2
    return 1
  fi
  mkdir -p "$work/out" "$dest"
  case "$url" in
    *.whl)
      # The wheel holds exactly one versioned core library (libonnxruntime.so.<ver>
      # / libonnxruntime.<ver>.dylib) next to an unrelated providers_shared helper,
      # which the pattern does not match. Extract it flat, under the plain name.
      if ! command -v unzip >/dev/null 2>&1; then echo "fetch-onnxruntime: unzip is required for wheels" >&2; return 1; fi
      unzip -q -j -o "$file" "$member" -d "$work/out" || { echo "fetch-onnxruntime: ${member} not found in ${url##*/}" >&2; return 1; }
      local count
      count="$(find "$work/out" -type f | wc -l | tr -d ' ')"
      if [[ "$count" != "1" ]]; then echo "fetch-onnxruntime: expected one ${member} in ${url##*/}, found ${count}" >&2; return 1; fi
      cp "$(find "$work/out" -type f | head -1)" "$dest/$wanted" || return 1
      ;;
    *) cp "$file" "$dest/$wanted" || return 1 ;;
  esac
  chmod 755 "$dest/$wanted" 2>/dev/null || true
  echo "fetch-onnxruntime: wrote $dest/$wanted ($(wc -c <"$dest/$wanted" | tr -d ' ') bytes) from ${url##*/}"
  return 0
}

while read -r _ url sha256; do
  if try_source "$url" "$sha256"; then exit 0; fi
  echo "fetch-onnxruntime: source unusable, trying the next one" >&2
done < <(grep -E "^${target} " "$pin")

echo "fetch-onnxruntime: every pinned source for ${target} failed" >&2
exit 1
