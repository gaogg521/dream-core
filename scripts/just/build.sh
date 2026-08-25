#!/usr/bin/env bash
set -euo pipefail

mode="${1:-release}"
shift || true

force=false
# Everything else (e.g. `--features enterprise` from `just build-enterprise`)
# is a real cargo flag and must reach `cargo build`, not just steer this
# script's own cache check below.
cargo_flags=()
for flag in "$@"; do
    if [[ "$flag" == "--force" || "$flag" == "-f" ]]; then
        force=true
    else
        cargo_flags+=("$flag")
    fi
done

sha256_file() {
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    elif command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        python3 -c 'import hashlib, sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' "$1"
    fi
}

case "$mode" in
    release)
        if ((${#cargo_flags[@]})); then
            just _cargo build --release "${cargo_flags[@]}"
        else
            just _cargo build --release
        fi
        binary="target/release/dreamcore"
        sum_file="target/.build-sum"
        label="Build"
        ;;
    debug)
        if ((${#cargo_flags[@]})); then
            just _cargo build "${cargo_flags[@]}"
        else
            just _cargo build
        fi
        binary="target/debug/dreamcore"
        sum_file="target/.build-debug-sum"
        label="Debug build"
        ;;
    *)
        echo "unknown build mode: $mode" >&2
        exit 1
        ;;
esac

new_sum=$(sha256_file "$binary")
old_sum=""
if [[ -f "$sum_file" && "$force" == "false" ]]; then
    old_sum=$(cat "$sum_file")
fi

if [[ "$new_sum" == "$old_sum" ]]; then
    echo
    echo "$label unchanged (sha256: ${new_sum:0:16})"
else
    echo "$new_sum" > "$sum_file"
    echo
    echo "$label complete (sha256: ${new_sum:0:16})"
fi
