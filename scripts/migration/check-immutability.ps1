$ErrorActionPreference = "Stop"

$repoRoot = (git rev-parse --show-toplevel)
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
Set-Location $repoRoot

# Migrations live under every crate that owns schema, not one directory:
# dream-core-db plus each dream-domain-* crate. This script used to point at a
# single pre-rebrand path that no longer exists, so it silently never ran — which
# is how a sweep managed to edit eleven published migrations unnoticed.
$migrationGlob = "crates/*/migrations*/*.sql"
$migrationDirs = Get-ChildItem -LiteralPath (Join-Path $repoRoot "crates") -Directory |
    ForEach-Object {
        @(
            (Join-Path $_.FullName "migrations"),
            (Join-Path $_.FullName "migrations_mysql")
        )
    } |
    Where-Object { Test-Path -LiteralPath $_ -PathType Container }

# The version is the first run of digits in the file name, so both `007_foo.sql`
# and `billing_007_bar.sql` resolve to 7. Files with no digits are ignored.
$duplicateVersions = foreach ($dir in $migrationDirs) {
    $byVersion = Get-ChildItem -LiteralPath $dir -File -Filter "*.sql" |
        ForEach-Object {
            if ($_.Name -match '([0-9]+)') {
                [PSCustomObject]@{ Version = [int64]$Matches[1]; Name = $_.Name }
            }
        } |
        Group-Object Version |
        Where-Object { $_.Count -gt 1 }

    foreach ($group in $byVersion) {
        [PSCustomObject]@{
            Dir     = $dir.Substring($repoRoot.Length).TrimStart('\', '/')
            Version = $group.Name
            Names   = ($group.Group | ForEach-Object { $_.Name }) -join ", "
        }
    }
}

if ($duplicateVersions) {
    [Console]::Error.WriteLine("Duplicate database migration versions are not allowed.")
    [Console]::Error.WriteLine("")
    [Console]::Error.WriteLine("Rename the later migration to the next unused numeric prefix within its directory.")
    [Console]::Error.WriteLine("")
    [Console]::Error.WriteLine("Duplicate versions:")
    foreach ($duplicate in $duplicateVersions) {
        [Console]::Error.WriteLine("$($duplicate.Dir) version $($duplicate.Version): $($duplicate.Names)")
    }
    exit 1
}

if ($env:DREAM_ALLOW_MAIN_MIGRATION_EDIT -eq "1") {
    Write-Output "DREAM_ALLOW_MAIN_MIGRATION_EDIT=1; skipping migration immutability check"
    exit 0
}

$baseRef = $env:DREAM_MIGRATION_BASE_REF
if ([string]::IsNullOrWhiteSpace($baseRef)) {
    git rev-parse --verify --quiet origin/main | Out-Null
    if ($LASTEXITCODE -eq 0) {
        $baseRef = "origin/main"
    } else {
        git rev-parse --verify --quiet main | Out-Null
        if ($LASTEXITCODE -eq 0) {
            $baseRef = "main"
        } else {
            Write-Output "No origin/main or main ref found; skipping migration immutability check"
            exit 0
        }
    }
}

git rev-parse --verify --quiet $baseRef | Out-Null
if ($LASTEXITCODE -ne 0) {
    Write-Error "Migration immutability base ref not found: $baseRef"
    exit 1
}

$baseCommit = git merge-base HEAD $baseRef
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

$changed = git diff --name-status --diff-filter=DMR $baseCommit -- $migrationGlob
if (-not [string]::IsNullOrWhiteSpace(($changed -join "`n"))) {
    [Console]::Error.WriteLine("Existing migration files from main must not be modified or deleted.")
    [Console]::Error.WriteLine("")
    [Console]::Error.WriteLine("Fix this by reverting changes to existing migration files and adding a new next-numbered migration instead.")
    [Console]::Error.WriteLine("If this is an intentional high-risk exception, rerun with DREAM_ALLOW_MAIN_MIGRATION_EDIT=1.")
    [Console]::Error.WriteLine("")
    [Console]::Error.WriteLine("Changed existing migrations:")
    [Console]::Error.WriteLine(($changed -join "`n"))
    exit 1
}

Write-Output "Migration immutability check passed"
