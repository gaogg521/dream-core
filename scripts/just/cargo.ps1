$ErrorActionPreference = "Stop"

$CargoArgs = @($args)
$cargoConfig = @()
$restoreCargoLock = $false
$cargoLockSnapshot = $null
$dreamEngineRoot = $null
$crates = @()

function Invoke-Native {
    param(
        [Parameter(Mandatory = $true)]
        [string] $Command,
        [string[]] $Arguments = @()
    )

    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        $script:status = $LASTEXITCODE
        exit $LASTEXITCODE
    }
}

function Test-GitDiffClean {
    param([string[]] $Arguments)

    & git @Arguments | Out-Null
    return $LASTEXITCODE -eq 0
}

function Resolve-LocalPath {
    param([string] $Path)

    return [System.IO.Path]::GetFullPath($Path).TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
}

function Test-DreamEnginePatch {
    $metadataJson = & cargo @cargoConfig metadata --format-version 1
    if ($LASTEXITCODE -ne 0) {
        $script:status = $LASTEXITCODE
        exit $LASTEXITCODE
    }
    $metadata = $metadataJson | ConvertFrom-Json

    foreach ($crate in $crates) {
        $expectedPath = Resolve-LocalPath (Join-Path $dreamEngineRoot "crates/$crate")
        $package = $metadata.packages | Where-Object { $_.name -eq $crate } | Select-Object -First 1
        $actualPath = if ($null -eq $package) {
            "package not found"
        } else {
            Resolve-LocalPath (Split-Path -Parent $package.manifest_path)
        }

        if ($actualPath -ne $expectedPath) {
            Write-Error "DREAM_ENGINE patch was not used for $crate.`n  resolved: $actualPath`n  expected: $expectedPath"
            $script:status = 1
            exit 1
        }
    }
}

$status = 0
try {
    if (-not [string]::IsNullOrWhiteSpace($env:DREAM_ENGINE)) {
        if (-not (Test-Path -LiteralPath $env:DREAM_ENGINE -PathType Container)) {
            Write-Error "DREAM_ENGINE does not exist or is not a directory: $env:DREAM_ENGINE"
            exit 1
        }

        $dreamEngineRoot = (Resolve-Path -LiteralPath $env:DREAM_ENGINE).ProviderPath
        $crates = @(
            "dream-engine-agent",
            "dream-engine-compact",
            "dream-engine-config",
            "dream-engine-mcp",
            "dream-engine-memory",
            "dream-engine-process",
            "dream-engine-protocol",
            "dream-engine-providers",
            "dream-engine-skills",
            "dream-engine-tools",
            "dream-engine-types"
        )

        foreach ($crate in $crates) {
            $crateDir = Join-Path $dreamEngineRoot "crates/$crate"
            $manifest = Join-Path $crateDir "Cargo.toml"
            if (-not (Test-Path -LiteralPath $manifest -PathType Leaf)) {
                Write-Error "DREAM_ENGINE is missing ${crate}: $manifest"
                exit 1
            }

            $tomlPath = $crateDir.Replace("\", "/").Replace('"', '\"')
            $cargoConfig += @("--config", "patch.'https://github.com/gaogg521/dream-engine.git'.$crate.path = `"`"$tomlPath`"`"")
        }

        [Console]::Error.WriteLine("Using local dream_engine SDK: $dreamEngineRoot")

        if (Test-Path -LiteralPath "Cargo.lock" -PathType Leaf) {
            $cargoLockSnapshot = [System.IO.Path]::GetTempFileName()
            Copy-Item -LiteralPath "Cargo.lock" -Destination $cargoLockSnapshot -Force

            $worktreeClean = Test-GitDiffClean @("diff", "--quiet", "--", "Cargo.lock")
            $indexClean = Test-GitDiffClean @("diff", "--cached", "--quiet", "--", "Cargo.lock")
            if ($worktreeClean -and $indexClean) {
                $restoreCargoLock = $true
            } else {
                [Console]::Error.WriteLine("Cargo.lock already has changes; leaving successful DREAM_ENGINE lockfile updates in place.")
            }
        }

        [Console]::Error.WriteLine("Resolving Cargo.lock against local dream_engine SDK")
        $updateArgs = @($cargoConfig) + @(
            "update",
            "-p", "dream-engine-agent",
            "-p", "dream-engine-compact",
            "-p", "dream-engine-config",
            "-p", "dream-engine-mcp",
            "-p", "dream-engine-memory",
            "-p", "dream-engine-process",
            "-p", "dream-engine-protocol",
            "-p", "dream-engine-providers",
            "-p", "dream-engine-skills",
            "-p", "dream-engine-tools",
            "-p", "dream-engine-types"
        )
        Invoke-Native "cargo" $updateArgs
        Test-DreamEnginePatch
    }

    & cargo @cargoConfig @CargoArgs
    $status = $LASTEXITCODE
} finally {
    if ($null -ne $cargoLockSnapshot -and (Test-Path -LiteralPath $cargoLockSnapshot -PathType Leaf)) {
        if ($restoreCargoLock -or $status -ne 0) {
            Copy-Item -LiteralPath $cargoLockSnapshot -Destination "Cargo.lock" -Force
        }
        Remove-Item -LiteralPath $cargoLockSnapshot -Force
    }
}

exit $status
