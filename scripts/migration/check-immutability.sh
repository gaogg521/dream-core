#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

# Migrations live under every crate that owns schema, not one directory:
# dream-core-db plus each dream-domain-* crate. The pre-rebrand script only
# looked at the old `crates/aionui-db/migrations` path, which no longer
# exists — so this check silently never ran.
migration_glob='crates/*/migrations/*.sql'

allow_override="${DREAM_ALLOW_MAIN_MIGRATION_EDIT:-${AIONCORE_ALLOW_MAIN_MIGRATION_EDIT:-}}"
base_ref="${DREAM_MIGRATION_BASE_REF:-${AIONCORE_MIGRATION_BASE_REF:-}}"

# ── 1. No two migration files in the same directory may share a version ──
# The version is the first run of digits in the file name, so both
# `007_foo.sql` and `billing_007_bar.sql` resolve to 7. Files with no digits
# (hand-run fixtures) are ignored.
duplicate_versions="$(
    find crates -maxdepth 3 -type d -name migrations -print0 \
        | while IFS= read -r -d '' dir; do
            find "$dir" -maxdepth 1 -type f -name '*.sql' -printf '%f\n' 2>/dev/null \
                | awk -v dir="$dir" '
                    {
                        name = $0
                        version = name
                        gsub(/[^0-9].*$/, "", version)          # drop from the first non-digit
                        if (version == "") {
                            match(name, /[0-9]+/)                # …unless digits appear later
                            if (RSTART == 0) next
                            version = substr(name, RSTART, RLENGTH)
                        }
                        version += 0
                        count[version]++
                        files[version] = files[version] (files[version] == "" ? "" : ", ") name
                    }
                    END {
                        for (v in count) if (count[v] > 1) print dir " version " v ": " files[v]
                    }
                '
        done \
        | sort
)"

if [[ -n "$duplicate_versions" ]]; then
    cat >&2 <<'EOF'
Duplicate database migration versions are not allowed.

Rename the later migration to the next unused numeric prefix within its directory.

Duplicate versions:
EOF
    echo "$duplicate_versions" >&2
    exit 1
fi

# ── 2. Published migration files must not be modified or deleted ──
if [[ "$allow_override" == "1" ]]; then
    echo "DREAM_ALLOW_MAIN_MIGRATION_EDIT=1; skipping migration immutability check"
    exit 0
fi

if [[ -z "$base_ref" ]]; then
    if git rev-parse --verify --quiet origin/main >/dev/null; then
        base_ref="origin/main"
    elif git rev-parse --verify --quiet main >/dev/null; then
        base_ref="main"
    else
        echo "No origin/main or main ref found; skipping migration immutability check"
        exit 0
    fi
fi

if ! git rev-parse --verify --quiet "$base_ref" >/dev/null; then
    echo "Migration immutability base ref not found: $base_ref" >&2
    exit 1
fi

base_commit="$(git merge-base HEAD "$base_ref")"
changed="$(
    git diff --name-status --diff-filter=DMR "$base_commit" -- "$migration_glob"
)"

if [[ -n "$changed" ]]; then
    cat >&2 <<'EOF'
Existing migration files from main must not be modified or deleted.

Fix this by reverting changes to existing migration files and adding a new next-numbered migration instead.
If this is an intentional high-risk exception, rerun with DREAM_ALLOW_MAIN_MIGRATION_EDIT=1.

Changed existing migrations:
EOF
    echo "$changed" >&2
    exit 1
fi

echo "Migration immutability check passed"
