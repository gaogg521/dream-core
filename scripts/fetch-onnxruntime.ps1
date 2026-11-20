<#
.SYNOPSIS
  Windows counterpart of fetch-onnxruntime.sh: fetch the pinned official ONNX
  Runtime and place onnxruntime.dll in <DestDir>, plus the Visual C++ runtime
  DLLs it needs.

  onnxruntime-pin.txt lists one or more sources per target, tried in order, each
  pinned by sha256: a raw onnxruntime.dll (our own COS mirror) or an official
  PyPI wheel (a zip, from which the dll is extracted).

.EXAMPLE
  scripts/fetch-onnxruntime.ps1 -Target x86_64-pc-windows-msvc -DestDir target/onnxruntime
#>
param(
  [Parameter(Mandatory = $true)][string]$Target,
  [Parameter(Mandatory = $true)][string]$DestDir
)
$ErrorActionPreference = 'Stop'
$pin = Join-Path $PSScriptRoot 'onnxruntime-pin.txt'
$entries = @(Get-Content $pin | Where-Object { $_ -like "$Target *" })
if ($entries.Count -eq 0) {
  Write-Host "fetch-onnxruntime: no ONNX Runtime build is pinned for $Target; skipping"
  exit 0
}

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("ort-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $work | Out-Null

# Returns $true after writing $DestDir\onnxruntime.dll, or $false (having said why)
# so the caller can move on to the next source.
function Get-FromSource([string]$url, [string]$sha256) {
  $name = $url.Substring($url.LastIndexOf('/') + 1)
  $file = Join-Path $work 'download'
  Remove-Item -Force $file -ErrorAction SilentlyContinue
  Write-Host "fetch-onnxruntime: trying $name"
  try {
    Invoke-WebRequest -Uri $url -OutFile $file -UseBasicParsing
  }
  catch {
    Write-Warning "fetch-onnxruntime: download failed: $url ($($_.Exception.Message))"
    return $false
  }
  $actual = (Get-FileHash -Algorithm SHA256 $file).Hash.ToLowerInvariant()
  if ($actual -ne $sha256) {
    Write-Warning "fetch-onnxruntime: sha256 mismatch for ${name}: expected $sha256, got $actual"
    return $false
  }
  New-Item -ItemType Directory -Force -Path $DestDir | Out-Null
  $out = Join-Path $DestDir 'onnxruntime.dll'
  if ($url.EndsWith('.whl')) {
    # A wheel is a zip. Expand-Archive insists on a .zip extension, so open it
    # with .NET directly and pull out just the one library.
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [System.IO.Compression.ZipFile]::OpenRead($file)
    try {
      $hits = @($zip.Entries | Where-Object { $_.FullName -eq 'onnxruntime/capi/onnxruntime.dll' })
      if ($hits.Count -ne 1) {
        Write-Warning "fetch-onnxruntime: expected one onnxruntime/capi/onnxruntime.dll in $name, found $($hits.Count)"
        return $false
      }
      [System.IO.Compression.ZipFileExtensions]::ExtractToFile($hits[0], $out, $true)
    }
    finally { $zip.Dispose() }
  }
  else {
    Copy-Item $file $out -Force
  }
  Write-Host "fetch-onnxruntime: wrote $out ($((Get-Item $out).Length) bytes) from $name"
  return $true
}

try {
  $done = $false
  foreach ($entry in $entries) {
    $parts = $entry -split ' '
    if (Get-FromSource $parts[1] $parts[2]) { $done = $true; break }
    Write-Host 'fetch-onnxruntime: source unusable, trying the next one'
  }
  if (-not $done) { throw "every pinned source for $Target failed" }

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
