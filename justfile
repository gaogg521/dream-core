# Default: list available recipes
set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"]

cargo_script := if os_family() == "windows" { "powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/just/cargo.ps1" } else { "bash scripts/just/cargo.sh" }
build_script := if os_family() == "windows" { "powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/just/build.ps1" } else { "bash scripts/just/build.sh" }
install_script := if os_family() == "windows" { "powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/just/install.ps1" } else { "bash scripts/just/install.sh" }
migration_check_script := if os_family() == "windows" { "powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/migration/check-immutability.ps1" } else { "bash scripts/migration/check-immutability.sh" }
migration_check_test_script := if os_family() == "windows" { "powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/migration/check-immutability.test.ps1" } else { "bash scripts/migration/check-immutability.test.sh" }
auto_commit_script := if os_family() == "windows" { "powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/just/auto-commit-fixes.ps1" } else { "bash scripts/just/auto-commit-fixes.sh" }
update_aionrs_script := if os_family() == "windows" { "powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/just/update-aionrs.ps1" } else { "bash scripts/just/update-aionrs.sh" }
cat_config_script := if os_family() == "windows" { "powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File scripts/just/cat-config.ps1" } else { "bash scripts/just/cat-config.sh" }

default:
    @just --list

# Enable pre-commit hooks (run once after clone)
setup:
    git config core.hooksPath .githooks
    @echo "Git hooks enabled"

# Run cargo with optional local aionrs SDK patches.
_cargo *ARGS:
    @{{cargo_script}} {{ARGS}}

# Build in release mode (does not install; run `just install` for that)
# Use `just build --force` to skip cache check
build *FLAGS: lint-fix fmt
    @{{build_script}} release {{FLAGS}}

# Build in debug mode
# Use `just build-debug --force` to skip cache check
build-debug *FLAGS:
    @{{build_script}} debug {{FLAGS}}

# Build the enterprise edition (adds the governance plane: org / sso /
# enterprise / billing / platform).
#
# Same crate, same binary name — a cargo feature is a build-time choice, not a
# second target — so the two editions cannot coexist in `target/release`.
# Packaging renames the artifact; build one, ship it, then build the other.
#
#   just build                    -> dreamcore, personal edition
#   just build-enterprise         -> dreamcore, enterprise edition
#
# Verify which one you have: `cargo tree -p dream-core-app --edges normal`
# lists dream-domain-{org,sso,enterprise,billing,platform} only for the latter.
build-enterprise *FLAGS: lint-fix fmt
    @{{build_script}} release --features enterprise {{FLAGS}}

# Check that BOTH editions still compile. The personal edition is the one that
# regresses silently: a new `dream_domain_*` reference outside a
# `#[cfg(feature = "enterprise")]` block compiles fine for whoever added it and
# breaks only the build nobody runs locally.
check-editions:
    @just _cargo check -p dream-core-app
    @just _cargo check -p dream-core-app --features enterprise

# Build (if needed) then install the release binary to cargo bin
install: build
    @{{install_script}} release

# Run all tests
test:
    @just _cargo nextest run --workspace

# Run the MySQL-gated enterprise tests against a real MySQL 8.0.16+ server.
# They skip (pass vacuously) when DREAM_TEST_MYSQL_URL is unset, so this is
# always safe to run.
#
# PREFERRED: the native Windows MySQL (root/root on 127.0.0.1:3306) — no WSL
# networking involved (see dream-en/docs/wsl-testing-pitfalls.zh-CN.md):
#   DREAM_TEST_MYSQL_URL=mysql://root:root@127.0.0.1:3306/dream_test just test-mysql
#
# Fallback: a mysql:8 container inside WSL — works, but the WSL2 localhost
# port relay breaks after `wsl --shutdown` and needs the VM healthy:
#   docker run -d --name dream-mysql-test --restart unless-stopped #     -e MYSQL_ROOT_PASSWORD=test -p 13306:3306 mysql:8.0
#   DREAM_TEST_MYSQL_URL=mysql://root:test@localhost:13306/dream_test just test-mysql
test-mysql:
    @just _cargo nextest run -p dream-core-db -p dream-domain-org -p dream-domain-sso         -p dream-domain-enterprise -p dream-domain-billing -p dream-domain-platform         -p dream-domain-employee -p dream-domain-devops -p dream-domain-workflow         -p dream-domain-memory

# Ensure already-shipped database migrations stay immutable
migration-check:
    @{{migration_check_script}}

# Test the migration immutability guard itself
migration-check-test:
    @{{migration_check_test_script}}

# Lint (warnings = errors)
lint:
    @just _cargo clippy --workspace -- -D warnings

lint-fix:
    @just _cargo fix --allow-dirty --allow-staged
    @just _cargo clippy --fix --workspace --allow-dirty --allow-staged -- -D warnings

# Format code
fmt:
    @cargo fmt --all

# Check formatting (CI)
fmt-check:
    @cargo fmt --all -- --check

# Lint + format check + migration check + test
check: migration-check lint fmt-check test

# Run the server (debug)
run *ARGS:
    @just _cargo run --bin dreamcore -- {{ARGS}}

# Run the server (release)
run-release *ARGS:
    @just _cargo run --release --bin dreamcore -- {{ARGS}}

# Pre-push gate: migration check, format, lint, auto-commit fixes, test, then push
push *ARGS: migration-check lint-fix fmt _auto-commit-fixes test
    git push {{ARGS}}

# Auto-commit any formatting/lint fixes if there are changes
_auto-commit-fixes:
    @{{auto_commit_script}}

# Update aionrs dependency: bump Cargo.toml tag, then open a PR whose body
# carries aionrs feat/fix/perf as conventional footer for release-please.
# e.g. `just update-aionrs` (latest) or `just update-aionrs v0.2.9`
update-aionrs *TAG:
    @{{update_aionrs_script}} {{TAG}}

# Security audit
audit:
    @cargo audit

# Clean build artifacts
clean:
    @cargo clean

# Decode dev config and copy to clipboard when possible
cat-config:
    @{{cat_config_script}}
