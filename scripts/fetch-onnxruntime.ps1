<#
.SYNOPSIS
  Windows counterpart of fetch-onnxruntime.sh: download the pinned official ONNX
  Runtime and place onnxruntime.dll in <DestDir>, plus the Visual C++ runtime
  DLLs it needs.

.EXAMPLE
  scripts/fetch-onnxruntime.ps1 -Target x86_64-pc-windows-msvc -DestDir target/onnxruntime
#>
param(
  [Parameter(Mandatory = $true)][string]$Target,
  [Parameter(Mandatory = $true)][string]$DestDir
)
$ErrorActionPreference = 'Stop'
$pin = Join-Path $PSScriptRoot 'onnxruntime-pin.txt'
$lines = Get-Content $pin
$baseUrl = ($lines | Where-Object { $_ -like 'BASE_URL=*' } | Select-Object -First 1).Substring('BASE_URL='.Length)
$entry = $lines | Where-Object { $_ -like "$Target *" } | Select-Object -First 1
if (-not $entry) {
  Write-Host "fetch-onnxruntime: no ONNX Runtime build is pinned for $Target; skipping"
  exit 0
}
$parts = $entry -split ' '
$archive = $parts[1]
$sha256 = $parts[2]
if (-not $archive.EndsWith('.zip')) { throw "$archive is not a .zip; use fetch-onnxruntime.sh" }

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("ort-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $work | Out-Null
try {
  Write-Host "fetch-onnxruntime: downloading $archive"
  $zip = Join-Path $work $archive
  Invoke-WebRequest -Uri "$baseUrl/$archive" -OutFile $zip -UseBasicParsing
  $actual = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLowerInvariant()
  if ($actual -ne $sha256) { throw "sha256 mismatch for ${archive}: expected $sha256, got $actual" }

  Expand-Archive -LiteralPath $zip -DestinationPath $work -Force
  $dll = Get-ChildItem -Path $work -Recurse -Filter 'onnxruntime.dll' | Where-Object { $_.FullName -like '*\lib\*' } | Select-Object -First 1
  if (-not $dll) { throw "onnxruntime.dll not found inside $archive" }
  New-Item -ItemType Directory -Force -Path $DestDir | Out-Null
  Copy-Item $dll.FullName (Join-Path $DestDir 'onnxruntime.dll') -Force
  Write-Host "fetch-onnxruntime: wrote $DestDir\onnxruntime.dll ($($dll.Length) bytes)"

  # onnxruntime.dll is built against the dynamic Visual C++ runtime. dreamcore.exe
  # links the CRT statically, so a machine without the redistributable would
  # start the program fine and then fail to load the library. App-local copies of
  # these three DLLs are redistributable and remove that dependency.
  foreach ($name in 'vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll') {
    $source = Join-Path $env:WINDIR "System32\$name"
    if (Test-Path $source) { Copy-Item $source (Join-Path $DestDir $name) -Force }
    else { Write-Warning "fetch-onnxruntime: $name not found in System32; not bundled" }
  }
}
finally {
  Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
