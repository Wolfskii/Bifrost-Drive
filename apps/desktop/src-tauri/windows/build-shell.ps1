param(
  [string]$Target = 'x86_64-pc-windows-msvc'
)

# Builds the Explorer shell extension and stages it where the installer bundles it from.
# When BIFROST_SIGN_THUMBPRINT is set (release CI), the DLL is Authenticode-signed first, because
# it is loaded into explorer.exe and unsigned extensions are treated with suspicion.

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..\..')).Path

& cargo build --release -p bifrost-windows-shell --target $Target
if ($LASTEXITCODE -ne 0) { throw "cargo build failed for bifrost-windows-shell ($LASTEXITCODE)" }

$built = Join-Path $root "target\$Target\release\bifrost_windows_shell.dll"
if (-not (Test-Path $built)) { throw "The Explorer extension was not produced: $built" }

$staged = Join-Path $root '.cache\shell'
New-Item -ItemType Directory -Force -Path $staged | Out-Null
$dll = Join-Path $staged 'bifrost_windows_shell.dll'
Copy-Item $built $dll -Force

if (-not [string]::IsNullOrWhiteSpace($env:BIFROST_SIGN_THUMBPRINT)) {
  $signTool = $env:BIFROST_SIGNTOOL
  if ([string]::IsNullOrWhiteSpace($signTool)) {
    $sdkRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    $found = Get-ChildItem $sdkRoot -Filter signtool.exe -Recurse -File |
      Where-Object { $_.DirectoryName -like '*\x64' } |
      Sort-Object FullName -Descending |
      Select-Object -First 1
    if ($null -eq $found) { throw "Windows SDK signtool.exe was not found under $sdkRoot" }
    $signTool = $found.FullName
  }
  & $signTool sign /sha1 $env:BIFROST_SIGN_THUMBPRINT /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 $dll
  if ($LASTEXITCODE -ne 0) { throw "signtool failed for the Explorer extension ($LASTEXITCODE)" }
  Write-Host 'Signed the Explorer extension.'
}

Write-Host "Explorer extension staged at $dll"
