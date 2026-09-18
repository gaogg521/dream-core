#!/usr/bin/env bash
set -euo pipefail

cargo_config=()
restore_cargo_lock=false
cargo_lock_snapshot=""
dream_engine_root=""

restore_local_lockfile() {
    local status=$?

    if [[ -n "$cargo_lock_snapshot" && -f "$cargo_lock_snapshot" ]]; then
        if [[ "$restore_cargo_lock" == "true" || "$status" -ne 0 ]]; then
            cp "$cargo_lock_snapshot" Cargo.lock || status=$?
        fi
    fi
    if [[ -n "$cargo_lock_snapshot" ]]; then
        rm -f "$cargo_lock_snapshot"
    fi

    return "$status"
}
trap restore_local_lockfile EXIT

verify_local_dream_engine_patch() {
    local metadata_file
    metadata_file=$(mktemp)
    cargo "${cargo_config[@]}" metadata --format-version 1 > "$metadata_file"

    python3 - "$dream_engine_root" "$metadata_file" "${crates[@]}" <<'PY'
import json
import sys
from pathlib import Path

dream_engine_root = Path(sys.argv[1]).resolve()
metadata_path = Path(sys.argv[2])
crates = sys.argv[3:]
metadata = json.loads(metadata_path.read_text())
packages = {package["name"]: package for package in metadata["packages"]}

for crate in crates:
    package = packages.get(crate)
    expected = (dream_engine_root / "crates" / crate).resolve()
    if not package:
        print(f"DREAM_ENGINE patch was not used for {crate}.", file=sys.stderr)
        print("  resolved: package not found", file=sys.stderr)
        print(f"  expected: {expected}", file=sys.stderr)
        sys.exit(1)

    actual = Path(package["manifest_path"]).resolve().parent
    if actual != expected:
        print(f"DREAM_ENGINE patch was not used for {crate}.", file=sys.stderr)
        print(f"  resolved: {actual}", file=sys.stderr)
        print(f"  expected: {expected}", file=sys.stderr)
        sys.exit(1)
PY

    rm -f "$metadata_file"
}

if [[ -n "${DREAM_ENGINE:-}" ]]; then
    if [[ ! -d "$DREAM_ENGINE" ]]; then
        echo "DREAM_ENGINE does not exist or is not a directory: $DREAM_ENGINE" >&2
        exit 1
    fi

    dream_engine_root=$(cd "$DREAM_ENGINE" && pwd -P)
    crates=(
        dream-engine-agent
        dream-engine-compact
        dream-engine-config
        dream-engine-mcp
        dream-engine-memory
        dream-engine-process
        dream-engine-protocol
        dream-engine-providers
        dream-engine-skills
        dream-engine-tools
        dream-engine-types
    )

    for crate in "${crates[@]}"; do
        crate_dir="$dream_engine_root/crates/$crate"
        if [[ ! -f "$crate_dir/Cargo.toml" ]]; then
            echo "DREAM_ENGINE is missing $crate: $crate_dir/Cargo.toml" >&2
            exit 1
        fi

        toml_path=${crate_dir//\\/\\\\}
        toml_path=${toml_path//\"/\\\"}
        cargo_config+=(--config "patch.'https://github.com/gaogg521/dream-engine.git'.$crate.path = \"$toml_path\"")
    done

    echo "Using local dream_engine SDK: $dream_engine_root" >&2

    if [[ -f Cargo.lock ]]; then
        cargo_lock_snapshot=$(mktemp)
        cp Cargo.lock "$cargo_lock_snapshot"

        if git diff --quiet -- Cargo.lock && git diff --cached --quiet -- Cargo.lock; then
            restore_cargo_lock=true
        else
            echo "Cargo.lock already has changes; leaving successful DREAM_ENGINE lockfile updates in place." >&2
        fi
    fi

    echo "Resolving Cargo.lock against local dream_engine SDK" >&2
    cargo "${cargo_config[@]}" update \
        -p dream-engine-agent \
        -p dream-engine-compact \
        -p dream-engine-config \
        -p dream-engine-mcp \
        -p dream-engine-memory \
        -p dream-engine-process \
        -p dream-engine-protocol \
        -p dream-engine-providers \
        -p dream-engine-skills \
        -p dream-engine-tools \
        -p dream-engine-types
    verify_local_dream_engine_patch
fi

if ((${#cargo_config[@]})); then
    cargo "${cargo_config[@]}" "$@"
else
    cargo "$@"
fi
